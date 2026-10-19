"""Inspect and damage stored per-source records through the real shared store."""

from __future__ import annotations

import json
import sqlite3
from contextlib import closing
from typing import Any

from molt.cli.source_record_store import RecordKind, shared_source_records
from molt.cli.function_records import (
    decode_function_records,
    decode_lowering_record,
    encode_lowering_record,
)


def source_records(kind: RecordKind) -> dict[str, dict[str, Any]]:
    """Every stored ``kind`` record in the caller's isolated shared cache."""
    path = shared_source_records().path
    if not path.exists():
        return {}
    with closing(sqlite3.connect(path)) as connection:
        rows = connection.execute(
            "SELECT key, payload FROM records WHERE kind = ?", (kind,)
        ).fetchall()
    result = {}
    for key, payload in rows:
        if kind == "module-lowering":
            frame = decode_lowering_record(payload)
            value = json.loads(frame.metadata)
            functions, _seeds = decode_function_records(frame)
            value["result"]["functions"] = functions
        else:
            value = json.loads(payload)
        result[key] = value
    return result


def replace_source_record(kind: RecordKind, key: str, payload: dict[str, Any]) -> None:
    """Replace one record through its producer, preserving its generation."""
    store = shared_source_records()
    with closing(sqlite3.connect(store.path)) as connection:
        row = connection.execute(
            "SELECT generation FROM records WHERE kind = ? AND key = ?", (kind, key)
        ).fetchone()
    assert row is not None, f"no stored {kind} record {key}"
    if kind == "module-lowering":
        result = payload["result"]
        metadata = {
            **payload,
            "result": {
                name: value for name, value in result.items() if name != "functions"
            },
        }
        encoded, _seeds = encode_lowering_record(
            json.dumps(metadata).encode(), result["functions"]
        )
    else:
        encoded = json.dumps(payload).encode()
    assert store.put(kind, key, encoded, generation=row[0])


def delete_source_record(kind: RecordKind, key: str) -> None:
    """Remove one stored record, as an evicted or never-published one."""
    with (
        closing(sqlite3.connect(shared_source_records().path)) as connection,
        connection,
    ):
        deleted = connection.execute(
            "DELETE FROM records WHERE kind = ? AND key = ?", (kind, key)
        ).rowcount
    assert deleted == 1, f"no stored {kind} record {key}"
