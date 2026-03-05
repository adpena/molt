from __future__ import annotations

import json
import queue
import shutil
import subprocess
import threading
import time
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

from .logging_utils import log

try:  # pragma: no cover - optional dependency
    import duckdb as _duckdb_module
except Exception:  # pragma: no cover - optional dependency
    _duckdb: Any = None
else:
    _duckdb = _duckdb_module


class DurableMemoryStore:
    """Append-only durable telemetry with optional DuckDB/Parquet materialization."""

    def __init__(
        self,
        *,
        root: Path,
        sync_interval_seconds: int = 180,
        max_queue: int = 4096,
    ) -> None:
        self._root = root
        self._root.mkdir(parents=True, exist_ok=True)
        self._events_jsonl = self._root / "events.jsonl"
        self._duckdb_path = self._root / "events.duckdb"
        self._parquet_path = self._root / "events.parquet"
        self._sync_interval_seconds = max(int(sync_interval_seconds), 30)
        self._queue: queue.Queue[dict[str, Any]] = queue.Queue(maxsize=max_queue)
        self._stop_event = threading.Event()
        self._thread = threading.Thread(
            target=self._run,
            name="symphony-durable-memory",
            daemon=True,
        )
        self._duckdb_bin = shutil.which("duckdb")
        self._last_sync_monotonic = 0.0
        self._last_sync_utc: str | None = None
        self._dropped_rows = 0
        self._thread.start()

    def record(self, row: dict[str, Any]) -> None:
        payload = dict(row)
        payload.setdefault(
            "recorded_at",
            datetime.now(UTC).isoformat().replace("+00:00", "Z"),
        )
        try:
            self._queue.put_nowait(payload)
        except queue.Full:
            self._dropped_rows += 1
            if self._dropped_rows in {1, 10, 100} or self._dropped_rows % 1000 == 0:
                log(
                    "WARNING",
                    "durable_memory_queue_dropped",
                    dropped=self._dropped_rows,
                    root=str(self._root),
                )

    def close(self) -> None:
        self._stop_event.set()
        if self._thread.is_alive():
            self._thread.join(timeout=2.0)

    def summary(self, *, limit: int = 120) -> dict[str, Any]:
        files = {
            "jsonl": _file_snapshot(self._events_jsonl),
            "duckdb": _file_snapshot(self._duckdb_path),
            "parquet": _file_snapshot(self._parquet_path),
        }
        recent_events = _tail_jsonl(self._events_jsonl, limit=max(limit, 10))
        kind_counts: dict[str, int] = {}
        for row in recent_events:
            kind = str(row.get("kind") or "unknown")
            kind_counts[kind] = kind_counts.get(kind, 0) + 1
        return {
            "enabled": True,
            "root": str(self._root),
            "queue_depth": self._queue.qsize(),
            "dropped_rows": self._dropped_rows,
            "last_sync_utc": self._last_sync_utc,
            "files": files,
            "kind_counts": kind_counts,
            "recent_events": recent_events,
        }

    def _run(self) -> None:
        pending: list[dict[str, Any]] = []
        while not self._stop_event.is_set():
            try:
                row = self._queue.get(timeout=0.5)
                pending.append(row)
            except queue.Empty:
                pass

            if pending:
                self._append_jsonl(pending)
                pending.clear()

            now_mono = time.monotonic()
            if (
                now_mono - self._last_sync_monotonic
            ) >= self._sync_interval_seconds and self._events_jsonl.exists():
                self._sync_duckdb_parquet()
                self._last_sync_monotonic = now_mono

        while True:
            try:
                pending.append(self._queue.get_nowait())
            except queue.Empty:
                break
        if pending:
            self._append_jsonl(pending)
        if self._events_jsonl.exists():
            self._sync_duckdb_parquet()

    def _append_jsonl(self, rows: list[dict[str, Any]]) -> None:
        try:
            self._root.mkdir(parents=True, exist_ok=True)
            with self._events_jsonl.open("a", encoding="utf-8") as handle:
                for row in rows:
                    handle.write(json.dumps(row, ensure_ascii=True) + "\n")
        except Exception as exc:  # pragma: no cover - filesystem dependent
            log(
                "WARNING",
                "durable_memory_append_failed",
                root=str(self._root),
                error=str(exc),
            )

    def _sync_duckdb_parquet(self) -> None:
        if _duckdb is None and self._duckdb_bin is None:
            return
        sql = (
            "CREATE OR REPLACE TABLE events AS "
            "SELECT * FROM read_ndjson_auto("
            f"'{_sql_quote(str(self._events_jsonl))}', "
            "union_by_name=true, ignore_errors=true"
            "); "
            "COPY (SELECT * FROM events ORDER BY recorded_at) TO "
            f"'{_sql_quote(str(self._parquet_path))}' "
            "(FORMAT PARQUET, COMPRESSION ZSTD);"
        )
        if _duckdb is not None:
            try:
                conn = _duckdb.connect(str(self._duckdb_path))
                try:
                    conn.execute(sql)
                finally:
                    conn.close()
                self._last_sync_utc = (
                    datetime.now(UTC).isoformat().replace("+00:00", "Z")
                )
            except Exception as exc:  # pragma: no cover - duckdb dependent
                log(
                    "WARNING",
                    "durable_memory_sync_failed",
                    root=str(self._root),
                    error=str(exc),
                )
            return
        assert self._duckdb_bin is not None
        try:
            proc = subprocess.run(
                [self._duckdb_bin, str(self._duckdb_path), "-c", sql],
                check=False,
                capture_output=True,
                text=True,
                timeout=60,
            )
        except Exception as exc:  # pragma: no cover - subprocess dependent
            log(
                "WARNING",
                "durable_memory_sync_failed",
                root=str(self._root),
                error=str(exc),
            )
            return
        if proc.returncode != 0:
            log(
                "WARNING",
                "durable_memory_sync_nonzero",
                root=str(self._root),
                returncode=int(proc.returncode),
                stderr=(proc.stderr or "").strip()[:1000],
            )
            return
        self._last_sync_utc = datetime.now(UTC).isoformat().replace("+00:00", "Z")


def _file_snapshot(path: Path) -> dict[str, Any]:
    if not path.exists():
        return {"exists": False, "size_bytes": 0, "modified_at": None}
    stat = path.stat()
    modified = (
        datetime.fromtimestamp(stat.st_mtime, tz=UTC).isoformat().replace("+00:00", "Z")
    )
    return {"exists": True, "size_bytes": int(stat.st_size), "modified_at": modified}


def _tail_jsonl(path: Path, *, limit: int) -> list[dict[str, Any]]:
    if not path.exists():
        return []
    try:
        with path.open("rb") as handle:
            handle.seek(0, 2)
            end = handle.tell()
            pos = end
            block = 4096
            newlines = 0
            chunks: list[bytes] = []
            while pos > 0 and newlines <= limit * 2:
                read_size = min(block, pos)
                pos -= read_size
                handle.seek(pos)
                chunk = handle.read(read_size)
                chunks.insert(0, chunk)
                newlines += chunk.count(b"\n")
            content = b"".join(chunks).decode("utf-8", errors="replace")
    except Exception:
        return []
    rows: list[dict[str, Any]] = []
    for line in content.splitlines()[-limit:]:
        text = line.strip()
        if not text:
            continue
        try:
            parsed = json.loads(text)
        except json.JSONDecodeError:
            continue
        if isinstance(parsed, dict):
            rows.append(parsed)
    return rows


def _sql_quote(value: str) -> str:
    return value.replace("'", "''")
