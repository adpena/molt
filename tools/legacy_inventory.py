#!/usr/bin/env python3
"""Generated legacy inventory: the `legacy_count` authority for phase exits.

docs/design/CENTURY_SYSTEMS_PLAN.md §4.10 requires every release to inventory
legacy flags, aliases, shims, fallback paths, duplicate registries, dead gates,
and stale docs, and §5 makes `legacy_count == 0` part of every phase predicate.

The inventory is a registry (config/legacy_inventory.toml), not a heuristic
scan: each row names one legacy lane, the authority that superseded it, and a
`presence` probe (a path, optionally with a regex the file must still contain).
The count is the number of registered rows whose probe still matches. A row
whose probe no longer matches is *retired* and must be deleted from the
registry in the same landing (`--check` fails on retired rows so the registry
cannot silently rot into a list of ghosts).

Unregistered legacy lanes are not counted; they are the subject of review, and
the registry is the place a reviewer records them so the count becomes true.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import tomllib
from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
REGISTRY_PATH = ROOT / "config" / "legacy_inventory.toml"
SCHEMA = "molt.legacy-inventory.v1"
_ROW_KEYS = frozenset(
    {"id", "path", "superseded_by", "removal_release", "reason", "pattern"}
)
_REQUIRED_ROW_KEYS = frozenset(
    {"id", "path", "superseded_by", "removal_release", "reason"}
)


@dataclass(frozen=True)
class LegacyItem:
    id: str
    path: str
    superseded_by: str
    removal_release: str
    reason: str
    pattern: str | None

    def present(self, root: Path) -> bool:
        target = root / self.path
        if not target.exists():
            return False
        if self.pattern is None:
            return True
        if not target.is_file():
            raise ValueError(
                f"{self.id}: pattern probes require a file path: {self.path}"
            )
        text = target.read_text(encoding="utf-8", errors="strict")
        return re.search(self.pattern, text, re.MULTILINE) is not None


def load_registry(path: Path = REGISTRY_PATH) -> tuple[LegacyItem, ...]:
    with path.open("rb") as handle:
        document = tomllib.load(handle)
    if document.get("schema") != SCHEMA:
        raise ValueError(f"{path}: unsupported legacy inventory schema")
    rows = document.get("item", [])
    if not isinstance(rows, list):
        raise ValueError(f"{path}: item must be an array of tables")
    items: list[LegacyItem] = []
    seen: set[str] = set()
    for index, raw_row in enumerate(rows):
        if not isinstance(raw_row, Mapping):
            raise ValueError(f"{path}: item[{index}] must be a table")
        row: dict[str, Any] = {str(key): value for key, value in raw_row.items()}
        keys = set(row)
        if not _REQUIRED_ROW_KEYS <= keys or not keys <= _ROW_KEYS:
            raise ValueError(
                f"{path}: item[{index}] keys must be {sorted(_REQUIRED_ROW_KEYS)}"
                f" plus optional pattern; got {sorted(keys)}"
            )
        fields: dict[str, str] = {}
        for key in sorted(_REQUIRED_ROW_KEYS):
            value = row[key]
            if not isinstance(value, str) or not value.strip():
                raise ValueError(
                    f"{path}: item[{index}].{key} must be a non-empty string"
                )
            fields[key] = value
        pattern = row.get("pattern")
        if pattern is not None:
            if not isinstance(pattern, str) or not pattern:
                raise ValueError(
                    f"{path}: item[{index}].pattern must be a non-empty string"
                )
            re.compile(pattern)
        if fields["id"] in seen:
            raise ValueError(f"{path}: duplicate legacy item id {fields['id']!r}")
        seen.add(fields["id"])
        relative = Path(fields["path"])
        if relative.is_absolute() or ".." in relative.parts:
            raise ValueError(f"{path}: item[{index}].path must be repository-relative")
        items.append(
            LegacyItem(
                id=fields["id"],
                path=fields["path"],
                superseded_by=fields["superseded_by"],
                removal_release=fields["removal_release"],
                reason=fields["reason"],
                pattern=pattern,
            )
        )
    return tuple(items)


@dataclass(frozen=True)
class InventoryRow:
    id: str
    path: str
    superseded_by: str
    removal_release: str
    present: bool


@dataclass(frozen=True)
class Inventory:
    legacy_count: int
    retired: tuple[str, ...]
    items: tuple[InventoryRow, ...]

    def as_dict(self) -> dict[str, Any]:
        return {
            "schema": SCHEMA,
            "legacy_count": self.legacy_count,
            "retired": list(self.retired),
            "items": [vars(row) for row in self.items],
        }


def inventory(root: Path = ROOT, registry_path: Path | None = None) -> Inventory:
    items = load_registry(registry_path or (root / "config" / "legacy_inventory.toml"))
    rows = tuple(
        InventoryRow(
            id=item.id,
            path=item.path,
            superseded_by=item.superseded_by,
            removal_release=item.removal_release,
            present=item.present(root),
        )
        for item in items
    )
    return Inventory(
        legacy_count=sum(1 for row in rows if row.present),
        retired=tuple(row.id for row in rows if not row.present),
        items=rows,
    )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=(__doc__ or "").splitlines()[0])
    parser.add_argument("--root", type=Path, default=ROOT)
    parser.add_argument(
        "--json", action="store_true", help="emit the inventory as JSON"
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="fail when a registered row no longer matches (delete it from the registry)",
    )
    args = parser.parse_args(argv)
    report = inventory(args.root.resolve())
    if args.json:
        print(json.dumps(report.as_dict(), indent=2, sort_keys=True))
    else:
        for row in report.items:
            state = "PRESENT" if row.present else "RETIRED"
            print(f"{state:8} {row.id}  {row.path}  -> {row.superseded_by}")
        print(
            f"legacy_inventory: legacy_count={report.legacy_count} retired={len(report.retired)}"
        )
    if args.check and report.retired:
        print(
            "legacy_inventory: retired rows must be deleted from config/legacy_inventory.toml: "
            + ", ".join(report.retired),
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
