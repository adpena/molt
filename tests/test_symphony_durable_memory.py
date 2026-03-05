from __future__ import annotations

from pathlib import Path

import molt.symphony.durable_memory as durable_memory
from molt.symphony.durable_memory import DurableMemoryStore


def _wait_for(condition, timeout_s: float = 3.0) -> None:  # type: ignore[no-untyped-def]
    import time

    start = time.monotonic()
    while time.monotonic() - start < timeout_s:
        if condition():
            return
        time.sleep(0.05)
    raise AssertionError("condition not reached")


def test_durable_memory_backup_and_integrity(tmp_path: Path) -> None:
    store = DurableMemoryStore(root=tmp_path, sync_interval_seconds=3600, max_queue=128)
    try:
        store.record(
            {
                "kind": "manual_action",
                "issue_identifier": "MOL-1",
                "message": "ok",
            }
        )
        _wait_for(lambda: (tmp_path / "events.jsonl").exists())

        integrity = store.run_integrity_check()
        assert integrity["ok"] is True
        assert "jsonl_readable" in integrity["checks"]

        backup = store.create_backup(reason="test")
        assert backup["ok"] is True
        backup_dir = Path(str(backup["backup_dir"]))
        assert (backup_dir / "metadata.json").exists()
        assert (backup_dir / "events.jsonl").exists()

        listed = store.list_backups(limit=5)
        assert listed
        assert listed[0]["name"] == backup_dir.name
    finally:
        store.close()


def test_durable_memory_prune_backups(tmp_path: Path) -> None:
    store = DurableMemoryStore(root=tmp_path, sync_interval_seconds=3600, max_queue=128)
    try:
        store.record({"kind": "codex_event", "issue_identifier": "MOL-2"})
        _wait_for(lambda: (tmp_path / "events.jsonl").exists())
        first = store.create_backup(reason="a")
        second = store.create_backup(reason="b")
        assert Path(str(first["backup_dir"])).exists()
        assert Path(str(second["backup_dir"])).exists()

        # Force retention pruning by keep_latest=1 and max_age_days=0-equivalent (1 day)
        # while timestamps are current; should retain both unless keep_latest excludes older.
        result = store.prune_backups(keep_latest=1, max_age_days=0)
        assert result["ok"] is True
        remaining = store.list_backups(limit=10)
        assert len(remaining) >= 1
    finally:
        store.close()


def test_durable_memory_integrity_detects_bad_jsonl(tmp_path: Path) -> None:
    (tmp_path / "events.jsonl").write_text(
        '{"kind":"ok"}\nnot-json\n', encoding="utf-8"
    )
    store = DurableMemoryStore(root=tmp_path, sync_interval_seconds=3600, max_queue=128)
    try:
        check = store.run_integrity_check()
        assert check["ok"] is False
        jsonl_check = check["checks"]["jsonl_readable"]
        assert jsonl_check["ok"] is False
    finally:
        store.close()


def test_durable_memory_duckdb_lock_conflict_is_warning(
    tmp_path: Path, monkeypatch
) -> None:
    class _LockedDuckDB:
        def connect(self, *_args, **_kwargs):  # type: ignore[no-untyped-def]
            raise RuntimeError(
                'IO Error: Could not set lock on file "events.duckdb": '
                "Conflicting lock is held in python (PID 123)"
            )

    (tmp_path / "events.duckdb").write_bytes(b"not-used")
    store = DurableMemoryStore(root=tmp_path, sync_interval_seconds=3600, max_queue=128)
    monkeypatch.setattr(durable_memory, "_duckdb", _LockedDuckDB())
    try:
        check = store._check_duckdb_readable()
        assert check["ok"] is True
        assert check["reason"] == "duckdb_locked_by_writer"
        assert check["warning"] is True
    finally:
        store.close()


def test_durable_memory_profiling_baseline_aggregates_checkpoints(
    tmp_path: Path,
) -> None:
    store = DurableMemoryStore(root=tmp_path, sync_interval_seconds=3600, max_queue=128)
    try:
        store.record(
            {
                "kind": "profiling_checkpoint",
                "latencies": {
                    "tick": {"count": 10, "avg_ms": 4.0, "p95_ms": 8.0, "max_ms": 10.0},
                    "turn": {
                        "count": 2,
                        "avg_ms": 20.0,
                        "p95_ms": 30.0,
                        "max_ms": 35.0,
                    },
                },
            }
        )
        store.record(
            {
                "kind": "profiling_checkpoint",
                "latencies": {
                    "tick": {
                        "count": 12,
                        "avg_ms": 6.0,
                        "p95_ms": 10.0,
                        "max_ms": 14.0,
                    },
                },
            }
        )
        _wait_for(lambda: (tmp_path / "events.jsonl").exists())
        _wait_for(
            lambda: (
                store.profiling_baseline(
                    max_events=200, min_samples=1, max_labels=8
                ).get("checkpoint_samples", 0)
                >= 2
            )
        )
        baseline = store.profiling_baseline(max_events=200, min_samples=1, max_labels=8)
        assert baseline["checkpoint_samples"] >= 2
        tick = baseline["by_label"]["tick"]
        assert tick["samples"] >= 2
        assert tick["avg_ms"] >= 5.0
        assert "turn" in baseline["by_label"]
    finally:
        store.close()
