#!/usr/bin/env python3
"""Fail closed on duplicate unmangled symbols across runtime satellite crates.

The root runtime crate still carries feature-off fallback copies for several
stdlib areas while satellite crates carry feature-on authorities. Those pairs
are governed by `check_satellite_parity.py` and Cargo feature cfgs.

Satellite crates, however, are linked together in `stdlib_full`. A symbol
defined by two satellites is not a fallback pair; it is a link-time collision
and a split authority. This guard enforces one `#[no_mangle] extern "C"` owner
per symbol across `runtime/molt-runtime-*` satellite crates.
"""

from __future__ import annotations

import argparse
from collections import defaultdict
from dataclasses import dataclass
import json
from pathlib import Path
import re
import sys
from typing import Iterable


REPO_ROOT = Path(__file__).resolve().parents[1]
RUNTIME_ROOT = REPO_ROOT / "runtime"

NO_MANGLE_EXTERN_RE = re.compile(
    r"#\[(?:unsafe\()?no_mangle\)?\]\s*"
    r"(?:\n\s*#\[[^\n]+\]\s*)*"
    r"\n\s*pub\s+(?:unsafe\s+)?extern\s+\"C\"\s+fn\s+"
    r"(?P<name>[A-Za-z_][A-Za-z0-9_]*)",
    re.MULTILINE,
)


@dataclass(frozen=True, slots=True)
class SymbolOwner:
    symbol: str
    crate: str
    path: str
    line: int


def _satellite_src_roots(runtime_root: Path) -> Iterable[Path]:
    for src in sorted(runtime_root.glob("molt-runtime-*/src")):
        if src.is_dir():
            yield src


def _is_shipped_source(path: Path) -> bool:
    if path.name == "test_host.rs":
        return False
    return "tests" not in path.parts


def _line_number(text: str, offset: int) -> int:
    return text.count("\n", 0, offset) + 1


def collect_symbol_owners(
    runtime_root: Path = RUNTIME_ROOT,
) -> dict[str, list[SymbolOwner]]:
    owners: dict[str, list[SymbolOwner]] = defaultdict(list)
    for src_root in _satellite_src_roots(runtime_root):
        crate = src_root.parent.name
        for path in sorted(src_root.rglob("*.rs")):
            if not _is_shipped_source(path):
                continue
            text = path.read_text(encoding="utf-8")
            rel_path = path.relative_to(runtime_root.parent).as_posix()
            for match in NO_MANGLE_EXTERN_RE.finditer(text):
                owners[match.group("name")].append(
                    SymbolOwner(
                        symbol=match.group("name"),
                        crate=crate,
                        path=rel_path,
                        line=_line_number(text, match.start()),
                    )
                )
    return dict(owners)


def find_cross_crate_collisions(
    owners: dict[str, list[SymbolOwner]],
) -> dict[str, list[SymbolOwner]]:
    collisions: dict[str, list[SymbolOwner]] = {}
    for symbol, symbol_owners in sorted(owners.items()):
        crates = {owner.crate for owner in symbol_owners}
        if len(crates) > 1:
            collisions[symbol] = sorted(
                symbol_owners, key=lambda owner: (owner.crate, owner.path, owner.line)
            )
    return collisions


def _json_payload(collisions: dict[str, list[SymbolOwner]]) -> dict[str, object]:
    return {
        "ok": not collisions,
        "collisions": {
            symbol: [
                {
                    "symbol": owner.symbol,
                    "crate": owner.crate,
                    "path": owner.path,
                    "line": owner.line,
                }
                for owner in owners
            ]
            for symbol, owners in collisions.items()
        },
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--json",
        action="store_true",
        help="emit machine-readable collision details",
    )
    args = parser.parse_args(argv)

    collisions = find_cross_crate_collisions(collect_symbol_owners())
    if args.json:
        print(json.dumps(_json_payload(collisions), indent=2, sort_keys=True))
    if not collisions:
        if not args.json:
            print("runtime satellite symbol owners OK")
        return 0

    if not args.json:
        print(
            "runtime satellite symbol owner collision(s) found:",
            file=sys.stderr,
        )
        for symbol, owners in collisions.items():
            print(f"- {symbol}", file=sys.stderr)
            for owner in owners:
                print(f"  {owner.crate}: {owner.path}:{owner.line}", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
