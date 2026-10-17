"""Inspect and damage stored per-source records through the real store."""

from __future__ import annotations

import json
import sqlite3
from contextlib import closing
from pathlib import Path
from typing import Any

from molt.cli import module_graph_cache


def _store_paths(project_root: Path) -> tuple[Path, ...]:
    """The project's own store, then the toolchain's shared stdlib store."""
    return tuple(
        store.path for store in module_graph_cache._source_record_stores(project_root)
    )


def source_records(project_root: Path, kind: str) -> dict[str, dict[str, Any]]:
    """Every stored ``kind`` record a build of ``project_root`` reads, by key."""
    records: dict[str, dict[str, Any]] = {}
    for path in _store_paths(project_root):
        if not path.exists():
            continue
        with closing(sqlite3.connect(path)) as connection:
            rows = connection.execute(
                "SELECT key, payload FROM records WHERE kind = ?", (kind,)
            ).fetchall()
        records.update({key: json.loads(payload) for key, payload in rows})
    return records


def _update_one(project_root: Path, statement: str, arguments: tuple) -> int:
    changed = 0
    for path in _store_paths(project_root):
        if not path.exists():
            continue
        with closing(sqlite3.connect(path)) as connection, connection:
            changed += connection.execute(statement, arguments).rowcount
    return changed


def replace_source_record(
    project_root: Path, kind: str, key: str, payload: dict[str, Any]
) -> None:
    """Overwrite one stored record, keeping its compiler generation."""
    updated = _update_one(
        project_root,
        "UPDATE records SET payload = ? WHERE kind = ? AND key = ?",
        (json.dumps(payload).encode(), kind, key),
    )
    assert updated == 1, f"no stored {kind} record {key}"


def delete_source_record(project_root: Path, kind: str, key: str) -> None:
    """Remove one stored record, as an evicted or never-published one."""
    deleted = _update_one(
        project_root, "DELETE FROM records WHERE kind = ? AND key = ?", (kind, key)
    )
    assert deleted == 1, f"no stored {kind} record {key}"
