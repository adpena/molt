"""Inspect and damage stored per-source records through their real stores.

Import scans and module analyses are immutable record files
(``molt.cli.frontend_record_files``); intrinsic facts and lowerings are rows in
the shared SQLite store."""

from __future__ import annotations

import json
import sqlite3
from contextlib import closing
from typing import Any, cast

from molt.cli.function_records import (
    decode_function_records,
    decode_lowering_record,
    encode_lowering_record,
)
from molt.cli.frontend_record_files import (
    FrontendRecord,
    FrontendRecordFiles,
    FrontendRecordKind,
    frontend_record_files,
)
from molt.cli.source_record_store import (
    RecordKind,
    SourceRecordStore,
    shared_source_records,
)

_FILE_KINDS: frozenset[str] = frozenset(("import-scan", "module-analysis"))


def _saved_files() -> FrontendRecordFiles:
    """This process's record files after it published its queued index."""
    files = frontend_record_files()
    assert files.save()
    return files


def _file_digests(kind: str) -> dict[str, str]:
    """``kind``'s keys and digests as a fresh reader of the files sees them."""
    fresh = FrontendRecordFiles(_saved_files().cache_root)
    return {key: digest for (k, key), digest in fresh.addresses().items() if k == kind}


def _file_record(
    files: FrontendRecordFiles, kind: FrontendRecordKind, key: str
) -> FrontendRecord:
    record = files.get(kind, key, max_bytes=2**31)
    assert record is not None, f"no stored {kind} record {key}"
    return record


def _saved_store() -> SourceRecordStore:
    """The shared store after this process's queued records are saved.

    Inside a frontend operation a put is queued; the database holds it only
    after a save, so raw inspection saves first.
    """
    store = shared_source_records()
    store.save_pending(final=True)
    return store


def source_records(
    kind: RecordKind | FrontendRecordKind,
) -> dict[str, dict[str, Any]]:
    """Every stored ``kind`` record in the caller's isolated shared cache."""
    if kind in _FILE_KINDS:
        file_kind = cast(FrontendRecordKind, kind)
        files = _saved_files()
        return {
            key: json.loads(_file_record(files, file_kind, key).payload)
            for key in _file_digests(file_kind)
        }
    path = _saved_store().path
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


def replace_source_record(
    kind: RecordKind | FrontendRecordKind, key: str, payload: dict[str, Any]
) -> None:
    """Replace one record through its producer, preserving its generation."""
    if kind in _FILE_KINDS:
        assert key in _file_digests(kind), f"no stored {kind} record {key}"
        files = _saved_files()
        file_kind = cast(FrontendRecordKind, kind)
        assert files.put(file_kind, key, json.dumps(payload).encode())
        assert files.save()
        return
    store = _saved_store()
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


def delete_source_record(kind: RecordKind | FrontendRecordKind, key: str) -> None:
    """Remove one stored record, as an evicted or never-published one."""
    if kind in _FILE_KINDS:
        record = _file_record(_saved_files(), cast(FrontendRecordKind, kind), key)
        assert record.path is not None and record.payload
        with record.path.open("r+b") as stream:
            stream.seek(record.offset)
            stream.write(bytes((record.payload[0] ^ 1,)))
        return
    with (
        closing(sqlite3.connect(_saved_store().path)) as connection,
        connection,
    ):
        deleted = connection.execute(
            "DELETE FROM records WHERE kind = ? AND key = ?", (kind, key)
        ).rowcount
    assert deleted == 1, f"no stored {kind} record {key}"
