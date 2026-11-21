"""HF-942: hot state on the home volume, byte budgets and the free-space floor.

Oracles are the filesystem and SQLite themselves: which paths a record
routes, which rows remain, which entries a sweep may take.
"""

from __future__ import annotations

import hashlib
import json
import os
from collections import namedtuple
from pathlib import Path

import pytest

from molt import default_paths


@pytest.fixture(autouse=True)
def _fresh_hot_states(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(default_paths, "_HOT_STATES", {})


def _bind(artifact: Path, root: Path, budget: int = 1 << 20) -> None:
    root.mkdir(parents=True, exist_ok=True)
    (artifact / default_paths.HOT_STATE_RECORD).write_text(
        json.dumps(
            {
                "schema": default_paths.HOT_STATE_SCHEMA,
                "root": str(root),
                "budget_bytes": budget,
            }
        ),
        encoding="utf-8",
    )


def test_a_record_moves_locks_and_digests_under_the_same_relative_paths(
    tmp_path: Path,
) -> None:
    artifact = tmp_path / "artifact"
    cache = artifact / ".molt_cache"
    cache.mkdir(parents=True)
    # No record: everything stays below the artifact root.
    assert default_paths.cache_control_root(cache) == cache / "locks"
    assert default_paths.file_digest_cache_path(cache) == (
        cache / "file-digests" / "v3.sqlite"
    )
    default_paths._HOT_STATES.clear()
    hot = tmp_path / "home" / "hot"
    _bind(artifact, hot)
    assert default_paths.cache_control_root(cache) == hot / ".molt_cache" / "locks"
    assert default_paths.file_digest_cache_path(cache) == (
        hot / ".molt_cache" / "file-digests" / "v3.sqlite"
    )
    assert default_paths.hot_path(artifact, "target", "x.lock") == (
        hot / "target" / "x.lock"
    )


@pytest.mark.parametrize(
    "record",
    [
        "{",
        json.dumps({"schema": "other", "root": "/x", "budget_bytes": 1}),
        json.dumps(
            {"schema": default_paths.HOT_STATE_SCHEMA, "root": "rel", "budget_bytes": 1}
        ),
        json.dumps(
            {"schema": default_paths.HOT_STATE_SCHEMA, "root": "/x", "budget_bytes": 0}
        ),
    ],
)
def test_a_malformed_record_is_an_error_not_a_silent_fallback(
    tmp_path: Path, record: str
) -> None:
    (tmp_path / default_paths.HOT_STATE_RECORD).write_text(record, encoding="utf-8")
    with pytest.raises(ValueError):
        default_paths.hot_state(tmp_path)


def test_provisioning_binds_only_another_volume(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from tools import molt_dev_storage as storage

    artifact = tmp_path / "artifact"
    home = tmp_path / "home"
    artifact.mkdir()
    home.mkdir()
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: home))
    # One volume: no record.
    (row,) = storage.provision_hot_state(artifact, apply=True)
    assert row["action"] == "same-volume"
    assert not (artifact / default_paths.HOT_STATE_RECORD).exists()
    real_stat = os.stat

    def stat(
        path: os.PathLike[str] | str,
        *,
        dir_fd: int | None = None,
        follow_symlinks: bool = True,
    ) -> os.stat_result:
        result = real_stat(path, dir_fd=dir_fd, follow_symlinks=follow_symlinks)
        if Path(path) == home:
            fields = list(result)
            fields[2] = result.st_dev + 1  # st_dev
            return os.stat_result(fields)
        return result

    monkeypatch.setattr(storage.os, "stat", stat)
    (row,) = storage.provision_hot_state(artifact, apply=False)
    assert row["action"] == "bindable"
    assert not (artifact / default_paths.HOT_STATE_RECORD).exists()
    (row,) = storage.provision_hot_state(artifact, apply=True)
    assert row["action"] == "bound"
    state = default_paths.hot_state(artifact)
    assert state is not None and state.root.parent == home / ".molt" / "hot-state"
    assert state.root.is_dir() and state.budget_bytes == storage.HOT_STATE_BUDGET_BYTES


def test_the_floor_takes_day_idle_entries_only_while_the_volume_is_low(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    from molt import disk_capacity

    Usage = namedtuple("Usage", "total used free")
    free = [5]
    monkeypatch.setattr(
        disk_capacity.shutil,
        "disk_usage",
        lambda _path: Usage(100, 100 - free[0], free[0]),
    )
    assert disk_capacity.below_free_space_floor(tmp_path)
    free[0] = 25
    assert not disk_capacity.below_free_space_floor(tmp_path)


def test_cache_retention_trims_the_hot_root_when_its_volume_is_low(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt import cache_eviction
    from molt.cli.frontend_record_files import FrontendRecordFiles

    artifact = tmp_path / "artifact"
    cache = artifact / ".molt_cache"
    cache.mkdir(parents=True)
    hot = tmp_path / "home" / "hot"
    _bind(artifact, hot, budget=1 << 30)
    files = FrontendRecordFiles(cache)
    assert files.put("import-scan", "a" * 24, b"[1]")
    blob = files.blob_path(hashlib.sha256(b"[1]").hexdigest())
    assert blob.is_relative_to(hot)
    outside = cache / "unrelated.bin"
    outside.write_bytes(b"x" * 10)
    Usage = namedtuple("Usage", "total used free")
    free = [default_paths.HOT_STATE_MIN_FREE_BYTES * 2]
    monkeypatch.setattr(
        cache_eviction.shutil, "disk_usage", lambda _path: Usage(1 << 50, 0, free[0])
    )
    kwargs = dict(max_bytes=1 << 40, max_age_days=None, dry_run=False)
    cache_eviction._prune_locked(cache, **kwargs)
    assert blob.exists()
    free[0] = default_paths.HOT_STATE_MIN_FREE_BYTES - 1
    cache_eviction._prune_locked(cache, **kwargs)
    assert not blob.exists() and outside.exists()
