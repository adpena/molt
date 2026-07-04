"""Safety locks for the artifact-SSD janitor.

The janitor deletes files, so its refusals and protections are the contract
that matters most: it must never operate on a dangerous root, and must never
select a live/registered/current/fresh path for deletion.
"""
from __future__ import annotations

import time
from pathlib import Path

import pytest

from tools import molt_ssd_janitor as jan


def test_resolve_root_refuses_drive_and_repo_roots(tmp_path: Path) -> None:
    # A one-component / drive / filesystem root is refused.
    with pytest.raises(SystemExit):
        jan._resolve_root(str(Path(tmp_path.anchor)), force=False)
    # The repo checkout itself is refused.
    with pytest.raises(SystemExit):
        jan._resolve_root(str(jan.REPO_ROOT), force=False)
    # A non-existent root is refused.
    with pytest.raises(SystemExit):
        jan._resolve_root(str(tmp_path / "does-not-exist"), force=False)


def test_gather_protects_registered_current_and_fresh(
    monkeypatch, tmp_path: Path
) -> None:
    root = tmp_path / "Molt"
    (root / "tmp").mkdir(parents=True)
    (root / "target" / "sessions").mkdir(parents=True)
    (root / "worktrees").mkdir()

    # An old orphaned worktree (candidate) and a registered one (protected).
    orphan_wt = root / "worktrees" / "orphan"
    registered_wt = root / "worktrees" / "live"
    orphan_wt.mkdir()
    registered_wt.mkdir()
    # A fresh tmp entry (protected by min-idle) and an old one (candidate).
    fresh_tmp = root / "tmp" / "fresh"
    old_tmp = root / "tmp" / "old"
    fresh_tmp.mkdir()
    old_tmp.mkdir()
    # Current + old session dirs.
    cur_session = root / "target" / "sessions" / "run-current"
    old_session = root / "target" / "sessions" / "run-old"
    cur_session.mkdir()
    old_session.mkdir()

    old = time.time() - 30 * 86400
    for p in (orphan_wt, old_tmp, old_session):
        import os

        os.utime(p, (old, old))

    plan = jan._gather(
        root,
        classes=("tmp", "sessions", "worktrees"),
        ages={"tmp": 3, "sessions": 3, "scratch": 21, "worktrees": 7, "cargo": 30},
        min_idle_hours=6.0,
        cache_cap_gb=30.0,
        current_session="run-current",
        registered={registered_wt.resolve()},
        guard_sessions=set(),
    )
    selected = {c.path.resolve() for c in plan.candidates}

    assert orphan_wt.resolve() in selected
    assert old_tmp.resolve() in selected
    assert old_session.resolve() in selected
    # Never the registered worktree, the current session, or a fresh entry.
    assert registered_wt.resolve() not in selected
    assert cur_session.resolve() not in selected
    assert fresh_tmp.resolve() not in selected
