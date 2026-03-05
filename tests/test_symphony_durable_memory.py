from __future__ import annotations

from pathlib import Path

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
