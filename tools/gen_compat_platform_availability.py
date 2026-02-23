#!/usr/bin/env python3
"""Generate CPython stdlib platform-availability matrix for compat docs.

Sources:
- docs/python_documentation/python-3.12-docs-text/library/*.txt
- docs/python_documentation/python-3.13-docs-text/library/*.txt
- docs/python_documentation/python-3.14-docs-text/library/*.txt
"""

from __future__ import annotations

import argparse
import datetime as _dt
import re
from dataclasses import dataclass
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
DOCS_ROOT = ROOT / "docs" / "python_documentation"
OUT_PATH = (
    ROOT
    / "docs"
    / "spec"
    / "areas"
    / "compat"
    / "surfaces"
    / "stdlib"
    / "stdlib_platform_availability.generated.md"
)
VERSIONS = ("3.12", "3.13", "3.14")

_AVAIL_RE = re.compile(r"^Availability:\s*(.+?)\s*$")


@dataclass(frozen=True)
class Row:
    module: str
    py312: str
    py313: str
    py314: str
    wasi: str
    emscripten: str
    os_hint: str


def _extract_availability(path: Path) -> str:
    try:
        text = path.read_text(encoding="utf-8")
    except FileNotFoundError:
        return ""
    for line in text.splitlines():
        m = _AVAIL_RE.match(line.strip())
        if m:
            return m.group(1)
    return ""


def _norm(value: str) -> str:
    return value if value else "-"


def _wasm_status(raw_values: list[str], token: str) -> str:
    lowered = " ".join(v.lower() for v in raw_values if v)
    if not lowered:
        return "unknown"
    if f"not {token}" in lowered:
        return "blocked"
    return "allowed_or_unspecified"


def _os_hint(raw_values: list[str]) -> str:
    lowered = " ".join(v.lower() for v in raw_values if v)
    if not lowered:
        return "unspecified"
    has_unix = "unix" in lowered
    has_windows = "windows" in lowered
    if has_unix and not has_windows:
        return "unix-biased"
    if has_windows and not has_unix:
        return "windows-biased"
    if has_windows and has_unix:
        return "multi-os"
    return "unspecified"


def _collect_rows() -> list[Row]:
    module_to_by_version: dict[str, dict[str, str]] = {}
    for version in VERSIONS:
        lib_dir = DOCS_ROOT / f"python-{version}-docs-text" / "library"
        if not lib_dir.exists():
            continue
        for fpath in sorted(lib_dir.glob("*.txt")):
            module = fpath.stem
            avail = _extract_availability(fpath)
            if not avail:
                continue
            module_to_by_version.setdefault(module, {})[version] = avail

    rows: list[Row] = []
    for module in sorted(module_to_by_version):
        by_ver = module_to_by_version[module]
        values = [by_ver.get(v, "") for v in VERSIONS]
        rows.append(
            Row(
                module=module,
                py312=_norm(by_ver.get("3.12", "")),
                py313=_norm(by_ver.get("3.13", "")),
                py314=_norm(by_ver.get("3.14", "")),
                wasi=_wasm_status(values, "wasi"),
                emscripten=_wasm_status(values, "emscripten"),
                os_hint=_os_hint(values),
            )
        )
    return rows


def _render(rows: list[Row]) -> str:
    generated_on = _dt.datetime.now(_dt.timezone.utc).strftime("%Y-%m-%d %H:%M:%SZ")
    blocked_wasi = sum(1 for r in rows if r.wasi == "blocked")
    blocked_emscripten = sum(1 for r in rows if r.emscripten == "blocked")

    lines: list[str] = []
    lines.append("# Stdlib Platform Availability (CPython 3.12-3.14)")
    lines.append("")
    lines.append("**Status:** Generated")
    lines.append(
        "**Source:** `docs/python_documentation/python-<version>-docs-text/library/*.txt`"
    )
    lines.append(f"**Generated on (UTC):** {generated_on}")
    lines.append("")
    lines.append("## Summary")
    lines.append(f"- Modules with explicit Availability metadata: `{len(rows)}`")
    lines.append(f"- WASI blocked (any lane): `{blocked_wasi}`")
    lines.append(f"- Emscripten blocked (any lane): `{blocked_emscripten}`")
    lines.append("")
    lines.append("## Matrix")
    lines.append("")
    lines.append(
        "| Module | py312 | py313 | py314 | wasm_wasi | wasm_emscripten | os_hint |"
    )
    lines.append("| --- | --- | --- | --- | --- | --- | --- |")
    for r in rows:
        lines.append(
            f"| `{r.module}` | {r.py312} | {r.py313} | {r.py314} | {r.wasi} | {r.emscripten} | {r.os_hint} |"
        )
    lines.append("")
    lines.append("## Notes")
    lines.append(
        "- `allowed_or_unspecified` means CPython docs did not explicitly ban that platform in the Availability line."
    )
    lines.append(
        "- This matrix is a reference input for Molt capability and cross-platform planning, not an automatic parity claim."
    )
    return "\n".join(lines) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--write", action="store_true", help="Write output file in-place"
    )
    args = parser.parse_args()

    rows = _collect_rows()
    output = _render(rows)

    if args.write:
        OUT_PATH.parent.mkdir(parents=True, exist_ok=True)
        OUT_PATH.write_text(output, encoding="utf-8")
        print(f"wrote {OUT_PATH}")
    else:
        print(output, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
