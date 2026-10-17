"""Inspect and damage stored per-source records through the real store."""

from __future__ import annotations

import json
import sqlite3
from contextlib import closing
from pathlib import Path
from typing import Any

from molt.cli import module_graph_cache


def source_records(project_root: Path, kind: str) -> dict[str, dict[str, Any]]:
    """Every stored ``kind`` record of ``project_root``'s state root, by key."""
    path = module_graph_cache._source_records(project_root).path
    if not path.exists():
        return {}
    with closing(sqlite3.connect(path)) as connection:
        rows = connection.execute(
            "SELECT key, payload FROM records WHERE kind = ?", (kind,)
        ).fetchall()
    return {key: json.loads(payload) for key, payload in rows}


def replace_source_record(
    project_root: Path, kind: str, key: str, payload: dict[str, Any]
) -> None:
    """Overwrite one stored record, keeping its compiler generation."""
    path = module_graph_cache._source_records(project_root).path
    with closing(sqlite3.connect(path)) as connection, connection:
        updated = connection.execute(
            "UPDATE records SET payload = ? WHERE kind = ? AND key = ?",
            (json.dumps(payload).encode(), kind, key),
        ).rowcount
    assert updated == 1, f"no stored {kind} record {key}"


def delete_source_record(project_root: Path, kind: str, key: str) -> None:
    """Remove one stored record, as an evicted or never-published one."""
    path = module_graph_cache._source_records(project_root).path
    with closing(sqlite3.connect(path)) as connection, connection:
        deleted = connection.execute(
            "DELETE FROM records WHERE kind = ? AND key = ?", (kind, key)
        ).rowcount
    assert deleted == 1, f"no stored {kind} record {key}"
