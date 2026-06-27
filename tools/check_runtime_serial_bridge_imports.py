#!/usr/bin/env python3
"""Fail closed on direct runtime/host imports in molt-runtime-serial.

`molt-runtime-serial` is the RuntimeVtable pilot: runtime-owned services enter
the satellite through `__molt_serial_get_vtable`, then through function pointers.
This guard prevents new direct host imports from silently splitting that
authority again.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import json
from pathlib import Path
import re
import sys
from typing import Iterable


REPO_ROOT = Path(__file__).resolve().parents[1]
SERIAL_SRC = REPO_ROOT / "runtime" / "molt-runtime-serial" / "src"

EXTERN_BLOCK_RE = re.compile(r"unsafe\s+extern\s+\"C\"\s*\{(?P<body>.*?)\}", re.DOTALL)
LINK_NAME_RE = re.compile(r"#\s*\[\s*link_name\s*=\s*\"(?P<name>[^\"]+)\"\s*\]")
WASM_IMPORT_MODULE_RE = re.compile(r"#\s*\[\s*link\s*\(\s*wasm_import_module\s*=")
IMPORT_DECL_RE = re.compile(
    r"(?P<attrs>(?:\s*#\[[^\n]+\]\s*)*)\s*"
    r"(?:pub\s+(?:\([^)]*\)\s*)?)?"
    r"fn\s+(?P<name>[A-Za-z_][A-Za-z0-9_]*)\s*\(",
    re.MULTILINE,
)

ALLOWED_IMPORTS = frozenset(
    {
        ("runtime/molt-runtime-serial/src/bridge.rs", "__molt_serial_get_vtable"),
        # Platform CRT import for localtime conversion. This is not a Molt
        # runtime/host capability; the vtable rule governs Molt-owned services.
        ("runtime/molt-runtime-serial/src/datetime.rs", "_mktime64"),
    }
)

MOLT_HOST_IMPORT_RE = re.compile(r"^molt_.*_host$")


@dataclass(frozen=True, slots=True)
class ImportViolation:
    path: str
    line: int
    symbol: str
    reason: str


def _line_number(text: str, offset: int) -> int:
    return text.count("\n", 0, offset) + 1


def _source_files(serial_src: Path) -> Iterable[Path]:
    yield from sorted(serial_src.rglob("*.rs"))


def _rel(path: Path) -> str:
    try:
        return path.relative_to(REPO_ROOT).as_posix()
    except ValueError:
        parts = path.parts
        if "runtime" in parts:
            return Path(*parts[parts.index("runtime") :]).as_posix()
        return path.as_posix()


def find_serial_bridge_import_violations(
    serial_src: Path = SERIAL_SRC,
) -> list[ImportViolation]:
    violations: list[ImportViolation] = []
    for path in _source_files(serial_src):
        text = path.read_text(encoding="utf-8")
        rel_path = _rel(path)
        for match in WASM_IMPORT_MODULE_RE.finditer(text):
            violations.append(
                ImportViolation(
                    path=rel_path,
                    line=_line_number(text, match.start()),
                    symbol="wasm_import_module",
                    reason="direct wasm import module in RuntimeVtable satellite",
                )
            )
        for block in EXTERN_BLOCK_RE.finditer(text):
            body = block.group("body")
            body_base = block.start("body")
            for decl in IMPORT_DECL_RE.finditer(body):
                attrs = decl.group("attrs") or ""
                link_match = LINK_NAME_RE.search(attrs)
                symbol = link_match.group("name") if link_match else decl.group("name")
                key = (rel_path, symbol)
                if MOLT_HOST_IMPORT_RE.match(symbol):
                    reason = "direct Molt host import bypasses RuntimeVtable"
                elif key not in ALLOWED_IMPORTS:
                    reason = "new serial extern import must route through RuntimeVtable"
                else:
                    continue
                violations.append(
                    ImportViolation(
                        path=rel_path,
                        line=_line_number(text, body_base + decl.start()),
                        symbol=symbol,
                        reason=reason,
                    )
                )
    return violations


def _json_payload(violations: list[ImportViolation]) -> dict[str, object]:
    return {
        "ok": not violations,
        "violations": [
            {
                "path": violation.path,
                "line": violation.line,
                "symbol": violation.symbol,
                "reason": violation.reason,
            }
            for violation in violations
        ],
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)

    violations = find_serial_bridge_import_violations()
    if args.json:
        print(json.dumps(_json_payload(violations), indent=2, sort_keys=True))
    if not violations:
        if not args.json:
            print("runtime serial bridge imports OK")
        return 0

    if not args.json:
        print("runtime serial bridge import violation(s) found:", file=sys.stderr)
        for violation in violations:
            print(
                f"- {violation.path}:{violation.line}: {violation.symbol}: "
                f"{violation.reason}",
                file=sys.stderr,
            )
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
