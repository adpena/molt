"""HF-942: hot state on the home volume, byte budgets and the free-space floor.

Oracles are the filesystem and SQLite themselves: which paths a record
routes, which rows remain, which entries a sweep may take.
"""

from __future__ import annotations

import hashlib
import json
import os
from collections import namedtuple
from collections.abc import Mapping
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
    assert scratch.scratch_roots(lane, env) == (former,)
    hot = tmp_path / "home" / "hot"
    _bind(artifact, hot)
    default_paths._HOT_STATES.clear()
    assert scratch.scratch_root(lane, env) == hot.resolve() / "tmp" / "gs" / "v2"
    assert scratch.scratch_roots(lane, env) == (hot / "tmp" / "gs" / "v2", former)
    # An explicit scratch storage root outranks the binding.
    ram = tmp_path / "ram"
    ram.mkdir()
    selected = {**env, "MOLT_SCRATCH_STORAGE": f"disk:{ram}"}
    root = scratch.scratch_root(lane, selected)
    assert root.is_relative_to(ram.resolve()) and root.parts[-2:] == ("gs", "v2")
    assert scratch.scratch_roots(lane, selected) == (root,)


@pytest.mark.parametrize(
    "scenario",
    ["spill", "same-volume", "both-short", "hot-unreadable", "explicit", "queued"],
)
def test_guard_admits_the_managed_artifact_volume_only_for_hot_pressure(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, scenario: str
) -> None:
    from molt import disk_capacity, temporary_artifacts as scratch

    source = tmp_path / "source"
    source.mkdir()
    artifact = tmp_path / "artifact"
    artifact.mkdir()
    hot = tmp_path / "hot"
    _bind(artifact, hot)
    hot_scratch = hot / "tmp" / "gs" / "v2"
    external_scratch = artifact / "tmp" / "gs" / "v2"
    state = artifact / "tmp" / "memory_guard"
    token = "a" * 32
    env = {
        "MOLT_EXT_ROOT": str(artifact),
        "MOLT_MEMORY_GUARD_TOKEN": token,
        "MOLT_MEMORY_GUARD_MARKER": str(state / "active" / "guard-a.json"),
    }
    first_probe = hot
    if scenario == "explicit":
        explicit = tmp_path / "explicit"
        explicit.mkdir()
        env["MOLT_SCRATCH_STORAGE"] = f"disk:{explicit}"
        first_probe = explicit
    elif scenario == "queued":
        env["MOLT_MEMORY_GUARD_STATE_ROOT"] = str(hot / "queued" / "guard")

    real_stat = Path.stat
    hot_free = default_paths.HOT_STATE_MIN_FREE_BYTES - 1
    observed = []

    def fixture_stat(path: Path, *, follow_symlinks: bool = True) -> os.stat_result:
        result = real_stat(path, follow_symlinks=follow_symlinks)
        if scenario == "same-volume" or path != hot:
            return result
        return os.stat_result((*result[:2], result.st_dev + 1, *result[3:]))

    def free(path: Path) -> int:
        observed.append(path)
        if path == hot:
            if scenario == "hot-unreadable":
                raise OSError("hot capacity probe failed")
            return 0 if scenario == "queued" else hot_free
        if path == artifact and scenario == "spill":
            return disk_capacity.DEFAULT_SCRATCH_BUDGET_BYTES
        return 0

    monkeypatch.setattr(Path, "stat", fixture_stat)
    monkeypatch.setattr(disk_capacity, "_default_measure_free_bytes", free)
    monkeypatch.setattr(disk_capacity, "sys", SimpleNamespace(platform="linux"))
    if scenario != "spill":
        with pytest.raises(disk_capacity.DiskCapacityError) as failure:
            scratch.acquire_guard_scratch(source, env)
        rejected = failure.value.rejected[0]
        assert rejected.requested_path == (
            external_scratch
            if scenario == "both-short"
            else scratch.scratch_root(source, env)
        )
        expected = [hot, artifact] if scenario == "both-short" else [first_probe]
        assert observed == expected
        assert not scratch.scratch_root(source, env).exists()
        assert not external_scratch.exists()
        return

    lease = scratch.acquire_guard_scratch(source, env)
    try:
        assert observed == [hot, artifact] and not hot_scratch.exists()
        assert lease.generation == external_scratch / token
        assert lease.target.parent == external_scratch
        assert (
            scratch.scratch_generation(token, {"target": str(lease.target)})
            == lease.generation
        )
        child_env = {**env, scratch.SCRATCH_ENV: str(lease.target)}
        assert scratch.guard_scratch(child_env) == lease.target
        hot_free += 1
        observed.clear()
        second = scratch.acquire_guard_scratch(
            source, {**env, "MOLT_MEMORY_GUARD_TOKEN": "b" * 32}
        )
        try:
            assert observed == [hot]
            assert second.generation == hot_scratch / ("b" * 32)
            assert scratch.guard_scratch(child_env) == lease.target
        finally:
            second.release()
    finally:
        lease.release()


def test_storage_trim_sees_both_managed_guard_roots_under_queue_custody(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from tools import molt_dev_storage as storage

    artifact = tmp_path / "artifact"
    artifact.mkdir()
    hot = tmp_path / "hot"
    _bind(artifact, hot)
    roots = [base / "tmp" / "gs" / "v2" for base in (hot, artifact)]
    for root in roots:
        root.mkdir(parents=True)
    source = tmp_path / "source"
    source.mkdir()
    monkeypatch.setattr(storage, "migrate", lambda *args, **kwargs: [])
    rows = storage.trim(
        source,
        {
            "MOLT_EXT_ROOT": str(artifact),
            "MOLT_MEMORY_GUARD_STATE_ROOT": str(tmp_path / "queue" / "guard"),
        },
        apply=False,
        policy_source=Path(__file__).resolve().parents[2],
        legacy_temp=tmp_path / "empty",
    )
    scratch_rows = [row for row in rows if row["class"] == "guard-scratch"]
    assert [(row["path"], row["action"], row["errors"]) for row in scratch_rows] == [
        (str(root), "reclaimable", []) for root in roots
    ]


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


def test_only_reusable_checkout_targets_follow_the_hot_record(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt import checkout_state, disk_capacity, dx

    monkeypatch.setattr(
        disk_capacity, "_default_measure_free_bytes", lambda _path: 80 << 30
    )

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


def test_hot_cargo_shortfall_selects_and_pins_the_artifact_target(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt import disk_capacity, dx

    source, artifact, hot = (tmp_path / name for name in ("source", "artifact", "hot"))
    source.mkdir()
    artifact.mkdir()
    _bind(artifact, hot)
    monkeypatch.setattr(
        disk_capacity,
        "_default_measure_free_bytes",
        lambda path: (30 if path.is_relative_to(hot) else 80) << 30,
    )
    env = {"MOLT_EXT_ROOT": str(artifact)}
    nominal = dx.cargo_target_dir_for_artifact_root(artifact, None, source_root=source)
    expected = artifact / "target" / "checkouts" / dx.checkout_component(source)
    selected = dx.RunContext(source).canonical_env(env, create_dirs=False)

    assert nominal.is_relative_to(hot)
    assert dx.project_cargo_target_dir(source, env) == expected
    assert selected["CARGO_TARGET_DIR"] == str(expected)
    assert selected["MOLT_DIFF_CARGO_TARGET_DIR"] == str(expected)
    assert not expected.exists()

    explicit = dx.RunContext(source).canonical_env(
        {**env, "CARGO_TARGET_DIR": str(nominal)}, create_dirs=False
    )
    assert explicit["CARGO_TARGET_DIR"] == str(nominal)


def test_landing_pins_shared_gates_to_one_artifact_volume_target(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt import disk_capacity, dx
    from tools import disk_guard, molt_dev
    from tools.molt_dev_policy import SelectedGate

    source, artifact, hot = (tmp_path / name for name in ("source", "artifact", "hot"))
    source.mkdir()
    (source / "Cargo.toml").write_text("[workspace]\n", encoding="utf-8")
    artifact.mkdir()
    _bind(artifact, hot)
    monkeypatch.setenv("MOLT_EXT_ROOT", str(artifact))
    for name in ("CARGO_TARGET_DIR", "MOLT_DIFF_CARGO_TARGET_DIR", "MOLT_SESSION_ID"):
        monkeypatch.delenv(name, raising=False)
    hot_free = 80 << 30
    artifact_free = 80 << 30
    reclaim_calls: list[Path] = []

    def measure(path: Path) -> int:
        return hot_free if path.is_relative_to(hot) else artifact_free

    def reclaim(
        minimum: float, *, root: Path, env: Mapping[str, str]
    ) -> disk_guard.ReclaimResult:
        reclaim_calls.append(root)
        pytest.fail("shared gate planning must not reclaim unrelated hot state")

    monkeypatch.setattr(disk_capacity, "_default_measure_free_bytes", measure)
    monkeypatch.setattr(disk_capacity, "_darwin_important_usage_bytes", measure)
    monkeypatch.setattr(disk_guard, "ensure_free", reclaim)
    nominal = dx.cargo_target_dir_for_artifact_root(artifact, None, source_root=source)
    external = artifact / "target" / "checkouts" / dx.checkout_component(source)
    gate = SelectedGate(("cargo", "test"), ("cargo",))

    read_only = molt_dev.gate_environment(
        source, session_id="inspect", create_dirs=False
    )
    assert read_only["CARGO_TARGET_DIR"] == str(nominal)
    assert reclaim_calls == []

    first = molt_dev.gate_environment(
        source, session_id="first", create_dirs=False, gates=(gate,)
    )
    assert (
        first["CARGO_TARGET_DIR"]
        == first["MOLT_DIFF_CARGO_TARGET_DIR"]
        == str(external)
    )
    assert not external.exists()

    hot_free = 30 << 30
    second = molt_dev.gate_environment(
        source, session_id="second", create_dirs=False, gates=(gate,)
    )
    assert second["CARGO_TARGET_DIR"] == first["CARGO_TARGET_DIR"]
    assert reclaim_calls == []

    monkeypatch.setenv("CARGO_TARGET_DIR", str(nominal))
    diff = artifact / "explicit-diff-target"
    monkeypatch.setenv("MOLT_DIFF_CARGO_TARGET_DIR", str(diff))
    pinned = molt_dev.gate_environment(
        source, session_id="explicit", create_dirs=False, gates=(gate,)
    )
    assert pinned["CARGO_TARGET_DIR"] == str(nominal)
    assert pinned["MOLT_DIFF_CARGO_TARGET_DIR"] == str(diff)
    assert reclaim_calls == []

    monkeypatch.delenv("CARGO_TARGET_DIR")
    artifact_free = 10 << 30
    with pytest.raises(disk_capacity.DiskCapacityError) as failure:
        molt_dev.gate_environment(
            source, session_id="exhausted", create_dirs=False, gates=(gate,)
        )
    assert failure.value.rejected[0].requested_path == external
    read_only = molt_dev.gate_environment(
        source, session_id="inspect-exhausted", create_dirs=False
    )
    assert read_only["CARGO_TARGET_DIR"] == str(nominal)
    assert reclaim_calls == []


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
