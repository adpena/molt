from __future__ import annotations

import hashlib
import json
import math
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
        self._last_integrity_check: dict[str, Any] | None = None
        self._last_backup: dict[str, Any] | None = None
        self._profiling_baseline_cache_ttl_seconds = 10.0
        self._profiling_baseline_cache: (
            tuple[float, tuple[int, int], dict[str, Any]] | None
        ) = None
        self._profiling_trends_cache_ttl_seconds = 5.0
        self._profiling_trends_cache: (
            tuple[float, tuple[int, int], dict[str, Any]] | None
        ) = None
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
        backups = self.list_backups(limit=10)
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
            "backups": backups,
            "last_backup": self._last_backup,
            "last_integrity_check": self._last_integrity_check,
            "kind_counts": kind_counts,
            "profiling_baseline": self.profiling_baseline(
                max_events=max(limit * 20, 400),
                max_labels=32,
            ),
            "recent_events": recent_events,
        }

    def profiling_baseline(
        self,
        *,
        max_events: int = 2400,
        min_samples: int = 3,
        max_labels: int = 64,
    ) -> dict[str, Any]:
        signature = _file_signature(self._events_jsonl)
        now_mono = time.monotonic()
        cached = self._profiling_baseline_cache
        if (
            cached is not None
            and cached[1] == signature
            and (now_mono - cached[0]) <= self._profiling_baseline_cache_ttl_seconds
        ):
            return dict(cached[2])
        rows = _tail_jsonl(self._events_jsonl, limit=max(max_events, 200))
        by_label: dict[str, dict[str, float]] = {}
        checkpoint_count = 0
        for row in rows:
            if not isinstance(row, dict):
                continue
            if str(row.get("kind") or "") != "profiling_checkpoint":
                continue
            latencies_raw = row.get("latencies")
            if not isinstance(latencies_raw, dict):
                continue
            checkpoint_count += 1
            for raw_label, raw_stats in latencies_raw.items():
                label = str(raw_label).strip()
                if not label:
                    continue
                stats = raw_stats if isinstance(raw_stats, dict) else {}
                agg = by_label.get(label)
                if agg is None:
                    agg = {
                        "samples": 0.0,
                        "avg_ms_sum": 0.0,
                        "p95_ms_sum": 0.0,
                        "max_ms_peak": 0.0,
                        "calls_sum": 0.0,
                        "total_ms_sum": 0.0,
                    }
                    by_label[label] = agg
                agg["samples"] += 1.0
                agg["avg_ms_sum"] += _coerce_float(stats.get("avg_ms"))
                agg["p95_ms_sum"] += _coerce_float(stats.get("p95_ms"))
                agg["max_ms_peak"] = max(
                    agg["max_ms_peak"],
                    _coerce_float(stats.get("max_ms")),
                )
                agg["calls_sum"] += max(_coerce_float(stats.get("count")), 0.0)
                agg["total_ms_sum"] += max(_coerce_float(stats.get("total_ms")), 0.0)
        labels: list[tuple[str, dict[str, Any]]] = []
        for label, agg in by_label.items():
            samples = int(agg["samples"])
            if samples < max(int(min_samples), 1):
                continue
            avg_ms = agg["avg_ms_sum"] / max(samples, 1)
            p95_ms = agg["p95_ms_sum"] / max(samples, 1)
            labels.append(
                (
                    label,
                    {
                        "samples": samples,
                        "avg_ms": round(avg_ms, 3),
                        "p95_ms": round(p95_ms, 3),
                        "max_ms": round(float(agg["max_ms_peak"]), 3),
                        "avg_calls": round(agg["calls_sum"] / max(samples, 1), 3),
                        "avg_total_ms": round(agg["total_ms_sum"] / max(samples, 1), 3),
                    },
                )
            )
        labels.sort(
            key=lambda item: (
                _coerce_float(item[1].get("p95_ms")),
                _coerce_float(item[1].get("avg_ms")),
                _coerce_float(item[1].get("avg_total_ms")),
            ),
            reverse=True,
        )
        top_labels = labels[: max(int(max_labels), 1)]
        result = {
            "generated_at": datetime.now(UTC).isoformat().replace("+00:00", "Z"),
            "checkpoint_samples": checkpoint_count,
            "labels_count": len(top_labels),
            "by_label": {label: row for label, row in top_labels},
        }
        self._profiling_baseline_cache = (now_mono, signature, result)
        return dict(result)

    def profiling_trends(
        self,
        *,
        max_events: int = 3600,
        max_points: int = 120,
        max_labels: int = 4,
    ) -> dict[str, Any]:
        signature = _file_signature(self._events_jsonl)
        now_mono = time.monotonic()
        cached = self._profiling_trends_cache
        if (
            cached is not None
            and cached[1] == signature
            and (now_mono - cached[0]) <= self._profiling_trends_cache_ttl_seconds
        ):
            return dict(cached[2])

        rows = _tail_jsonl(self._events_jsonl, limit=max(max_events, 200))
        checkpoints = [
            row
            for row in rows
            if isinstance(row, dict)
            and str(row.get("kind") or "") == "profiling_checkpoint"
        ]
        if not checkpoints:
            result = {
                "generated_at": datetime.now(UTC).isoformat().replace("+00:00", "Z"),
                "checkpoint_samples": 0,
                "points": [],
                "labels": [],
            }
            self._profiling_trends_cache = (now_mono, signature, result)
            return dict(result)

        selected = _downsample_rows(checkpoints, max_points=max(max_points, 8))
        points: list[dict[str, Any]] = []
        label_peaks: dict[str, float] = {}
        for row in selected:
            process = row.get("process")
            process_map = process if isinstance(process, dict) else {}
            counters = row.get("counters")
            counters_map = counters if isinstance(counters, dict) else {}
            point = {
                "at": (
                    str(row.get("recorded_at") or row.get("at") or "")
                    or datetime.now(UTC).isoformat().replace("+00:00", "Z")
                ),
                "rss_high_water_kb": int(
                    max(_coerce_float(process_map.get("rss_high_water_kb")), 0.0)
                ),
                "queue_depth_peak": int(
                    max(_coerce_float(row.get("queue_depth_peak")), 0.0)
                ),
                "events_processed": int(
                    max(_coerce_float(counters_map.get("events_processed")), 0.0)
                ),
            }
            points.append(point)
            latencies = row.get("latencies")
            if isinstance(latencies, dict):
                for raw_label, raw_stats in latencies.items():
                    label = str(raw_label).strip()
                    if not label:
                        continue
                    stats = raw_stats if isinstance(raw_stats, dict) else {}
                    label_peaks[label] = max(
                        label_peaks.get(label, 0.0),
                        _coerce_float(stats.get("p95_ms")),
                    )

        top_labels = [
            label
            for label, _peak in sorted(
                label_peaks.items(),
                key=lambda item: item[1],
                reverse=True,
            )[: max(int(max_labels), 1)]
        ]
        label_rows: list[dict[str, Any]] = []
        for label in top_labels:
            series: list[dict[str, Any]] = []
            for row in selected:
                latencies = row.get("latencies")
                latencies_map = latencies if isinstance(latencies, dict) else {}
                stats_raw = latencies_map.get(label)
                stats = stats_raw if isinstance(stats_raw, dict) else {}
                if not stats:
                    continue
                series.append(
                    {
                        "at": (
                            str(row.get("recorded_at") or row.get("at") or "")
                            or datetime.now(UTC).isoformat().replace("+00:00", "Z")
                        ),
                        "avg_ms": round(_coerce_float(stats.get("avg_ms")), 3),
                        "p95_ms": round(_coerce_float(stats.get("p95_ms")), 3),
                        "count": int(max(_coerce_float(stats.get("count")), 0.0)),
                    }
                )
            if not series:
                continue
            label_rows.append({"label": label, "series": series})

        result = {
            "generated_at": datetime.now(UTC).isoformat().replace("+00:00", "Z"),
            "checkpoint_samples": len(checkpoints),
            "points": points,
            "labels": label_rows,
        }
        self._profiling_trends_cache = (now_mono, signature, result)
        return dict(result)

    def create_backup(self, *, reason: str = "manual") -> dict[str, Any]:
        backups_root = self._root / "backups"
        backups_root.mkdir(parents=True, exist_ok=True)
        timestamp = datetime.now(UTC).strftime("%Y%m%dT%H%M%SZ")
        backup_dir = backups_root / f"{timestamp}-{_sanitize_segment(reason)}"
        backup_dir.mkdir(parents=True, exist_ok=False)
        copied: list[dict[str, Any]] = []
        for source in (self._events_jsonl, self._duckdb_path, self._parquet_path):
            if not source.exists():
                continue
            destination = backup_dir / source.name
            shutil.copy2(source, destination)
            copied.append(
                {
                    "name": source.name,
                    "size_bytes": int(destination.stat().st_size),
                    "sha256": _sha256_file(destination),
                }
            )
        payload = {
            "ok": True,
            "backup_dir": str(backup_dir),
            "reason": reason,
            "files": copied,
            "created_at": datetime.now(UTC).isoformat().replace("+00:00", "Z"),
        }
        (backup_dir / "metadata.json").write_text(
            json.dumps(payload, ensure_ascii=True, indent=2) + "\n",
            encoding="utf-8",
        )
        self._last_backup = payload
        return payload

    def run_integrity_check(self) -> dict[str, Any]:
        checks: dict[str, Any] = {
            "jsonl_readable": self._check_jsonl_readable(max_lines=500),
            "duckdb_readable": self._check_duckdb_readable(),
            "parquet_readable": self._check_parquet_readable(),
        }
        ok = all(bool(value.get("ok")) for value in checks.values())
        payload = {
            "ok": ok,
            "checked_at": datetime.now(UTC).isoformat().replace("+00:00", "Z"),
            "checks": checks,
        }
        self._last_integrity_check = payload
        return payload

    def list_backups(self, *, limit: int = 10) -> list[dict[str, Any]]:
        backups_root = self._root / "backups"
        if not backups_root.exists():
            return []
        rows: list[dict[str, Any]] = []
        for child in sorted(backups_root.iterdir(), reverse=True):
            if not child.is_dir():
                continue
            stat = child.stat()
            rows.append(
                {
                    "path": str(child),
                    "name": child.name,
                    "created_at": datetime.fromtimestamp(stat.st_mtime, tz=UTC)
                    .isoformat()
                    .replace("+00:00", "Z"),
                }
            )
            if len(rows) >= max(limit, 1):
                break
        return rows

    def prune_backups(
        self, *, keep_latest: int = 20, max_age_days: int = 30
    ) -> dict[str, Any]:
        backups_root = self._root / "backups"
        if not backups_root.exists():
            return {"ok": True, "deleted": 0, "kept": 0}
        now = time.time()
        keep_latest = max(int(keep_latest), 1)
        max_age_seconds = max(int(max_age_days), 1) * 86400
        candidates = [
            child
            for child in sorted(backups_root.iterdir(), reverse=True)
            if child.is_dir()
        ]
        deleted = 0
        for idx, child in enumerate(candidates):
            if idx < keep_latest:
                continue
            age_seconds = max(now - child.stat().st_mtime, 0.0)
            if age_seconds < max_age_seconds:
                continue
            shutil.rmtree(child, ignore_errors=True)
            deleted += 1
        return {
            "ok": True,
            "deleted": deleted,
            "kept": max(len(candidates) - deleted, 0),
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

    def _check_jsonl_readable(self, *, max_lines: int) -> dict[str, Any]:
        if not self._events_jsonl.exists():
            return {"ok": True, "reason": "missing"}
        lines = 0
        try:
            with self._events_jsonl.open("r", encoding="utf-8") as handle:
                for raw in handle:
                    if lines >= max_lines:
                        break
                    text = raw.strip()
                    if not text:
                        continue
                    parsed = json.loads(text)
                    if not isinstance(parsed, dict):
                        return {
                            "ok": False,
                            "reason": "non_object_json",
                            "line": lines + 1,
                        }
                    lines += 1
        except Exception as exc:
            return {"ok": False, "reason": "jsonl_parse_failed", "error": str(exc)}
        return {"ok": True, "lines_checked": lines}

    def _check_duckdb_readable(self) -> dict[str, Any]:
        if not self._duckdb_path.exists():
            return {"ok": True, "reason": "missing"}
        query = "SELECT COUNT(*) AS c FROM events"
        if _duckdb is not None:
            try:
                conn = _duckdb.connect(str(self._duckdb_path), read_only=True)
                try:
                    rows = conn.execute(query).fetchall()
                finally:
                    conn.close()
                count = int(rows[0][0]) if rows else 0
                return {"ok": True, "rows": count}
            except Exception as exc:  # pragma: no cover - duckdb dependent
                error = str(exc)
                if "Table with name events does not exist" in error:
                    # DuckDB file can appear before the first full sync has
                    # materialized the events table; treat as warning-only.
                    return {
                        "ok": True,
                        "reason": "duckdb_events_table_missing",
                        "warning": True,
                        "error": error,
                    }
                if "different configuration than existing connections" in error:
                    # The background sync thread can briefly hold a rw connection
                    # while this integrity probe opens read_only; this is a busy
                    # state, not data corruption.
                    return {
                        "ok": True,
                        "reason": "duckdb_busy_existing_connection",
                        "warning": True,
                        "error": error,
                    }
                if "Conflicting lock is held" in error or (
                    "Could not set lock on file" in error
                ):
                    # Active writer lock means the file is healthy but currently busy.
                    # Treat as a warning-only state so integrity checks don't flap.
                    return {
                        "ok": True,
                        "reason": "duckdb_locked_by_writer",
                        "warning": True,
                        "error": error,
                    }
                return {"ok": False, "reason": "duckdb_query_failed", "error": error}
        if self._duckdb_bin is None:
            return {"ok": True, "reason": "duckdb_unavailable"}
        try:
            proc = subprocess.run(
                [self._duckdb_bin, str(self._duckdb_path), "-c", query],
                check=False,
                capture_output=True,
                text=True,
                timeout=20,
            )
        except Exception as exc:
            return {"ok": False, "reason": "duckdb_cli_failed", "error": str(exc)}
        if proc.returncode != 0:
            return {
                "ok": False,
                "reason": "duckdb_cli_nonzero",
                "stderr": (proc.stderr or "").strip()[:500],
            }
        return {"ok": True}

    def _check_parquet_readable(self) -> dict[str, Any]:
        if not self._parquet_path.exists():
            return {"ok": True, "reason": "missing"}
        if _duckdb is None and self._duckdb_bin is None:
            return {"ok": True, "reason": "duckdb_unavailable"}
        query = (
            "SELECT COUNT(*) AS c FROM read_parquet("
            f"'{_sql_quote(str(self._parquet_path))}')"
        )
        if _duckdb is not None:
            try:
                conn = _duckdb.connect(":memory:")
                try:
                    rows = conn.execute(query).fetchall()
                finally:
                    conn.close()
                count = int(rows[0][0]) if rows else 0
                return {"ok": True, "rows": count}
            except Exception as exc:  # pragma: no cover - duckdb dependent
                return {
                    "ok": False,
                    "reason": "parquet_query_failed",
                    "error": str(exc),
                }
        assert self._duckdb_bin is not None
        try:
            proc = subprocess.run(
                [self._duckdb_bin, ":memory:", "-c", query],
                check=False,
                capture_output=True,
                text=True,
                timeout=20,
            )
        except Exception as exc:
            return {"ok": False, "reason": "parquet_cli_failed", "error": str(exc)}
        if proc.returncode != 0:
            return {
                "ok": False,
                "reason": "parquet_cli_nonzero",
                "stderr": (proc.stderr or "").strip()[:500],
            }
        return {"ok": True}


def _file_snapshot(path: Path) -> dict[str, Any]:
    if not path.exists():
        return {"exists": False, "size_bytes": 0, "modified_at": None}
    stat = path.stat()
    modified = (
        datetime.fromtimestamp(stat.st_mtime, tz=UTC).isoformat().replace("+00:00", "Z")
    )
    return {"exists": True, "size_bytes": int(stat.st_size), "modified_at": modified}


def _file_signature(path: Path) -> tuple[int, int]:
    try:
        stat = path.stat()
    except OSError:
        return (0, 0)
    return (int(stat.st_mtime_ns), int(stat.st_size))


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


def _downsample_rows(
    rows: list[dict[str, Any]], *, max_points: int
) -> list[dict[str, Any]]:
    if len(rows) <= max_points:
        return list(rows)
    stride = max(int(len(rows) / max_points), 1)
    sampled = [rows[idx] for idx in range(0, len(rows), stride)]
    if sampled and sampled[-1] is not rows[-1]:
        sampled[-1] = rows[-1]
    if len(sampled) > max_points:
        sampled = sampled[-max_points:]
    return sampled


def _sql_quote(value: str) -> str:
    return value.replace("'", "''")


def _sanitize_segment(value: str) -> str:
    text = value.strip().lower()
    if not text:
        return "manual"
    safe = "".join(ch if ch.isalnum() or ch in {"-", "_", "."} else "-" for ch in text)
    return safe.strip("-") or "manual"


def _sha256_file(path: Path) -> str:
    hasher = hashlib.sha256()
    with path.open("rb") as handle:
        while True:
            chunk = handle.read(8192)
            if not chunk:
                break
            hasher.update(chunk)
    return hasher.hexdigest()


def _coerce_float(value: Any) -> float:
    try:
        parsed = float(value)
    except (TypeError, ValueError):
        return 0.0
    if not math.isfinite(parsed):
        return 0.0
    return parsed
