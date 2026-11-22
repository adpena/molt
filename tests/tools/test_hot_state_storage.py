"""HF-942: hot state on the home volume, byte budgets and the free-space floor.

Oracles are the filesystem and SQLite themselves: which paths a record
routes, which rows remain, which entries a sweep may take.
"""

from __future__ import annotations

import hashlib
import json
from collections import namedtuple
from pathlib import Path
from types import SimpleNamespace

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


def test_provisioning_binds_only_another_volume(tmp_path: Path) -> None:
    from tools import molt_dev_storage as storage

    artifact = tmp_path / "artifact"
    home = tmp_path / "home"
    artifact.mkdir()
    home.mkdir()
    # One volume: no record.
    (row,) = storage.provision_hot_state(artifact, apply=True, home=home)
    assert row["action"] == "same-volume"
    assert not (artifact / default_paths.HOT_STATE_RECORD).exists()
    devices = {home: 1, artifact: 2}
    (row,) = storage.provision_hot_state(
        artifact, apply=False, home=home, device_of=devices.__getitem__
    )
    assert row["action"] == "bindable"
    assert not (artifact / default_paths.HOT_STATE_RECORD).exists()
    (row,) = storage.provision_hot_state(
        artifact, apply=True, home=home, device_of=devices.__getitem__
    )
    assert row["action"] == "bound"
    state = default_paths.hot_state(artifact)
    assert state is not None and state.root.parent == home / ".molt" / "hot-state"
    assert state.root.is_dir() and state.budget_bytes == storage.HOT_STATE_BUDGET_BYTES


def test_the_floor_takes_day_idle_entries_only_while_the_volume_is_low(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    from molt import disk_capacity

    Usage = namedtuple("Usage", "total used free")
    assert disk_capacity.below_free_space_floor(
        tmp_path, usage=lambda _path: Usage(100, 95, 5)
    )
    assert not disk_capacity.below_free_space_floor(
        tmp_path, usage=lambda _path: Usage(100, 75, 25)
    )


def test_guard_scratch_follows_the_hot_record_and_names_the_former_root(
    tmp_path: Path,
) -> None:
    from molt import temporary_artifacts as scratch

    lane = tmp_path / "source"
    lane.mkdir()
    artifact = tmp_path / "artifact"
    artifact.mkdir()
    env = {"MOLT_EXT_ROOT": str(artifact)}
    former = artifact.resolve() / "tmp" / "gs" / "v2"
    # No record: the generation namespace stays on the artifact volume.
    assert scratch.scratch_root(lane, env) == former
    assert scratch.former_scratch_root(lane, env) is None
    hot = tmp_path / "home" / "hot"
    _bind(artifact, hot)
    default_paths._HOT_STATES.clear()
    assert scratch.scratch_root(lane, env) == hot.resolve() / "tmp" / "gs" / "v2"
    assert scratch.former_scratch_root(lane, env) == former
    # An explicit scratch storage root outranks the binding.
    ram = tmp_path / "ram"
    ram.mkdir()
    selected = {**env, "MOLT_SCRATCH_STORAGE": f"disk:{ram}"}
    root = scratch.scratch_root(lane, selected)
    assert root.is_relative_to(ram.resolve()) and root.parts[-2:] == ("gs", "v2")
    assert scratch.former_scratch_root(lane, selected) is None


def test_stdlib_family_locks_stay_where_the_backend_takes_them(
    tmp_path: Path,
) -> None:
    # HF-970: the Rust backend names <cache_root>/locks/<family>.<xx>.lock and
    # does not read the hot record; a hot mirror for these split one lock in two.
    artifact = tmp_path / "artifact"
    cache = artifact / ".molt_cache"
    cache.mkdir(parents=True)
    _bind(artifact, tmp_path / "home" / "hot")
    key = "ab" + "0" * 62
    for select, family in (
        (default_paths.shared_stdlib_cache_entry, "shared-stdlib"),
        (default_paths.native_stdlib_archive_cache_entry, "native-stdlib-archive"),
        (default_paths.native_stdlib_object_cache_entry, "native-stdlib-object"),
    ):
        assert select(cache, key)[1] == cache / "locks" / f"{family}.ab.lock"
    # Python-only locks still follow the record.
    assert default_paths.shared_cache_lock_path(cache, "prune").is_relative_to(
        tmp_path / "home" / "hot"
    )


def test_only_reusable_checkout_targets_follow_the_hot_record(tmp_path: Path) -> None:
    from molt import checkout_state, dx

    source = tmp_path / "source"
    source.mkdir()
    artifact = tmp_path / "artifact"
    artifact.mkdir()
    component = dx.checkout_component(source.resolve())
    outside = dx.cargo_target_dir_for_artifact_root(artifact, None, source_root=source)
    assert outside == artifact.resolve() / "target" / "checkouts" / component
    hot = tmp_path / "home" / "hot"
    _bind(artifact, hot)
    default_paths._HOT_STATES.clear()
    placed = dx.cargo_target_dir_for_artifact_root(artifact, None, source_root=source)
    assert placed == hot / "target" / "checkouts" / component
    session = dx.cargo_target_dir_for_artifact_root(
        artifact, "proof-1", source_root=source
    )
    assert session == outside / "sessions" / "proof-1"
    env = {"MOLT_EXT_ROOT": str(artifact), "MOLT_SESSION_ID": "proof-1"}
    assert dx.project_cargo_target_dir(source, env) == session
    assert (
        dx.project_cargo_target_dir(source, {**env, "MOLT_SESSION_ID_GENERATED": "1"})
        == placed
    )

    displaced_session = placed / "sessions" / "old-proof"
    for path in (session, displaced_session):
        path.mkdir(parents=True)
    assert {
        state.path
        for state in checkout_state.checkout_state_roots(source, env)
        if state.kind == "cargo-target"
    } == {outside, placed, session, displaced_session}
    assert {
        state.path
        for state in checkout_state.stamp_checkout_state(source, env)
        if state.kind == "cargo-target"
    } == {outside, session}
    assert not (displaced_session / checkout_state.LAST_USE_STAMP).exists()
    # A plain clone keeps Cargo's own target.
    assert dx.cargo_target_dir_for_artifact_root(source, None, source_root=source) == (
        source.resolve() / "target"
    )


def test_daemon_sweep_reaches_sessions_on_both_target_volumes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt import dx
    from molt.build_state_layout import build_state_root
    from molt.cli import backend_execution

    source = tmp_path / "source"
    source.mkdir()
    artifact = tmp_path / "artifact"
    artifact.mkdir()
    hot = tmp_path / "hot"
    _bind(artifact, hot)
    monkeypatch.setenv("MOLT_EXT_ROOT", str(artifact))
    monkeypatch.delenv("MOLT_BUILD_STATE_DIR", raising=False)
    monkeypatch.setenv("CARGO_TARGET_DIR", str(source / "target"))
    component = dx.checkout_component(source.resolve())
    stale = []
    for location in (artifact, hot):
        target = location / "target" / "checkouts" / component / "sessions" / "proof"
        target.mkdir(parents=True)
        path = (
            build_state_root(
                project_root=source,
                cargo_target=target,
                environment={"MOLT_EXT_ROOT": str(artifact)},
            )
            / "backend_daemon"
            / "stale.pid"
        )
        path.parent.mkdir(parents=True)
        path.write_text("0\n", encoding="utf-8")
        stale.append(path)

    assert backend_execution._sweep_orphaned_backend_daemon_locks(source) == 2
    assert all(not path.exists() for path in stale)


@pytest.mark.parametrize("operation", ["build", "scratch"])
def test_hot_outputs_require_raw_home_headroom(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, operation: str
) -> None:
    from molt import disk_capacity

    artifact = tmp_path / "artifact"
    artifact.mkdir()
    hot = tmp_path / "hot"
    _bind(artifact, hot)
    env = {"MOLT_EXT_ROOT": str(artifact)}
    admit = (
        disk_capacity.require_build_capacity
        if operation == "build"
        else disk_capacity.require_scratch_capacity
    )
    floor = default_paths.HOT_STATE_MIN_FREE_BYTES
    physical_free = {hot: floor - 1, artifact: 1}
    estimates = []

    def purgeable_estimate(path: Path) -> int:
        estimates.append(path)
        return floor

    monkeypatch.setattr(disk_capacity, "sys", SimpleNamespace(platform="darwin"))
    monkeypatch.setattr(
        disk_capacity, "_default_measure_free_bytes", physical_free.__getitem__
    )
    monkeypatch.setattr(
        disk_capacity, "_darwin_important_usage_bytes", purgeable_estimate
    )

    outputs = [artifact / "proof", hot / "warm"]
    with pytest.raises(disk_capacity.DiskCapacityError) as failure:
        admit(outputs, env=env)
    external, internal = failure.value.diagnostic["probes"]
    assert external["available_bytes"] == floor
    assert internal["available_bytes"] == floor - 1
    assert internal["required_bytes"] == floor
    assert estimates == [artifact]
    assert all(not path.exists() for path in outputs)

    physical_free[hot] = floor
    assert all(probe.ready for probe in admit(outputs, env=env).probes)


def test_the_storage_write_check_admits_the_hot_mirror_and_nothing_else(
    tmp_path: Path,
) -> None:
    from molt import dx
    from molt import temporary_artifacts as scratch

    repo = tmp_path / "repo"
    repo.mkdir()
    artifact = tmp_path / "artifact"
    artifact.mkdir()
    hot = tmp_path / "home" / "hot"
    _bind(artifact, hot)
    env = {"MOLT_EXT_ROOT": str(artifact)}
    admitted = (
        scratch.scratch_root(repo, env) / ("0" * 32) / "lock",
        default_paths.hot_path(artifact, "target", ".disk_guard", "x", "a.lock"),
    )
    with dx.storage_write_check(repo, env, molt_only=False):
        for path in admitted:
            assert path.is_relative_to(hot)
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b"")
    with pytest.raises(dx.StorageWriteError, match="outside the storage authority"):
        with dx.storage_write_check(repo, env, molt_only=False):
            (hot / "undeclared").write_bytes(b"refused")


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
    free = [default_paths.HOT_STATE_MIN_FREE_BYTES * 2]
    monkeypatch.setattr(cache_eviction, "_hot_volume_free", lambda _root: free[0])
    kwargs = dict(max_bytes=1 << 40, max_age_days=None, dry_run=False)
    cache_eviction._prune_locked(cache, **kwargs)
    assert blob.exists()
    free[0] = default_paths.HOT_STATE_MIN_FREE_BYTES - 1
    cache_eviction._prune_locked(cache, **kwargs)
    assert not blob.exists() and outside.exists()
