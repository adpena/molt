from __future__ import annotations

from pathlib import Path

import tools.symphony_durable_admin as durable_admin


def test_backup_check_and_summary(tmp_path: Path) -> None:
    root = tmp_path / "durable"
    root.mkdir(parents=True, exist_ok=True)
    (root / "events.jsonl").write_text('{"kind":"ok"}\n', encoding="utf-8")

    rc = durable_admin.main(["--root", str(root), "backup", "--reason", "test"])
    assert rc == 0
    backups = sorted((root / "backups").iterdir())
    assert backups

    rc = durable_admin.main(["--root", str(root), "check"])
    assert rc == 0

    rc = durable_admin.main(["--root", str(root), "summary"])
    assert rc == 0


def test_restore_uses_latest_backup(tmp_path: Path) -> None:
    root = tmp_path / "durable"
    root.mkdir(parents=True, exist_ok=True)
    (root / "events.jsonl").write_text('{"kind":"first"}\n', encoding="utf-8")

    rc = durable_admin.main(["--root", str(root), "backup", "--reason", "test"])
    assert rc == 0

    (root / "events.jsonl").write_text('{"kind":"mutated"}\n', encoding="utf-8")
    rc = durable_admin.main(["--root", str(root), "restore"])
    assert rc == 0
    assert "first" in (root / "events.jsonl").read_text(encoding="utf-8")


def test_restore_fails_without_backups(tmp_path: Path) -> None:
    root = tmp_path / "durable"
    root.mkdir(parents=True, exist_ok=True)
    rc = durable_admin.main(["--root", str(root), "restore"])
    assert rc == 2
