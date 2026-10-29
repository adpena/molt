from __future__ import annotations

import json
import os
import shutil
from contextlib import contextmanager
from pathlib import Path

import pytest

from molt import file_locks, file_publication
from molt import temporary_artifacts as scratch
from molt.exact_json import canonical_json_sha256, read_exact, write_exact
from tests.process_guard_common import (
    current_thread_only,
    install_module_view,
    install_thread_module_view,
)


def _lease(root: Path, number: int = 1, *, storage: str | None = None):
    state = root / "tmp" / "memory_guard"
    env = {
        "MOLT_MEMORY_GUARD_STATE_ROOT": str(state),
        "MOLT_MEMORY_GUARD_TOKEN": f"{number:032x}",
        "MOLT_MEMORY_GUARD_MARKER": str(state / "active" / f"guard-{number}.json"),
    }
    if storage is not None:
        env["MOLT_SCRATCH_STORAGE"] = storage
    lease = scratch.acquire_guard_scratch(root, env)
    env[scratch.SCRATCH_ENV] = str(lease.target)
    return lease, env


def _read(path: Path):
    return read_exact(path, max_bytes=65536, label="test scratch receipt")


def _finish(lease, *, success=True, closed=True, timings=None, **kwargs):
    return scratch.finish_guard_scratch(
        lease,
        success=success,
        closed=closed,
        evidence={"authority": "fixture", "child_returncode": 0 if success else 1},
        timings=scratch.ScratchFinalizationTimings() if timings is None else timings,
        **kwargs,
    )


def _alternate_path(path: Path) -> str:
    if os.name == "nt":
        spelling = str(path)
        return (
            "\\\\?\\UNC\\" + spelling[2:]
            if spelling.startswith("\\\\")
            else "\\\\?\\" + spelling
        )
    return str(path.parent) + os.sep + "." + os.sep + path.name


@pytest.mark.parametrize("parent_alternate", [False, True])
@pytest.mark.parametrize("child_alternate", [False, True])
def test_allocation_identity_is_independent_of_path_spelling(
    tmp_path, parent_alternate, child_alternate
):
    root = Path(_alternate_path(tmp_path)) if parent_alternate else tmp_path
    lease, env = _lease(root)
    if child_alternate:
        for field in (
            "MOLT_MEMORY_GUARD_STATE_ROOT",
            "MOLT_MEMORY_GUARD_MARKER",
            scratch.SCRATCH_ENV,
        ):
            env[field] = _alternate_path(scratch.resolve_owned_path(Path(env[field])))
    else:
        env = {
            key: str(scratch.resolve_owned_path(Path(value)))
            if key != "MOLT_MEMORY_GUARD_TOKEN"
            else value
            for key, value in env.items()
        }
    try:
        assert scratch.guard_scratch(env) == lease.target
    finally:
        result = _finish(lease)
    assert result["state"] == "reclaimed"
    assert result["retention"]["errors"] == []
    assert not lease.target.exists()


@pytest.mark.parametrize("field", ["generation", "guard_marker", "target"])
def test_owner_path_identity_accepts_equivalent_receipt_spelling(tmp_path, field):
    lease, env = _lease(tmp_path)
    owner_path = lease.generation / "owner.json"
    owner = dict(lease.owner)
    owner[field] = _alternate_path(Path(owner[field]))
    write_exact(owner_path, owner)
    try:
        assert scratch.guard_scratch(env) == lease.target
        assert _read(owner_path) == owner
    finally:
        write_exact(owner_path, lease.owner)
        _finish(lease)


def test_retention_compares_canonical_paths_without_changing_receipt_digest(tmp_path):
    lease, _ = _lease(tmp_path)
    _finish(lease, success=False)
    terminal_path = lease.generation / "terminal.json"
    terminal = _read(terminal_path)
    terminal["generation"] = _alternate_path(lease.generation)
    terminal["target"] = _alternate_path(Path(terminal["target"]))
    owner = _read(lease.generation / "owner.json")
    owner["generation"] = _alternate_path(lease.generation)
    owner["terminal_digest"] = canonical_json_sha256(terminal)
    write_exact(terminal_path, terminal)
    write_exact(lease.generation / "owner.json", owner)
    write_exact(
        scratch._index_path(lease.generation),
        {"schema": scratch.SCHEMA, "terminal_digest": owner["terminal_digest"]},
    )
    result = scratch.reclaim_terminal_scratch(
        lease.generation.parent, retention=scratch.ScratchRetention(0, 0)
    )
    assert result["errors"] == []
    assert result["reclaimed"] == [str(lease.generation)]
    # The alternate spellings matched; reclamation then removed the receipts.
    assert not lease.generation.exists()
    assert not scratch._index_path(lease.generation).exists()


def test_parent_allocation_binds_consumption_and_terminal_cleanup(tmp_path):
    lease, env = _lease(tmp_path)
    legacy = lease.target.parent / "pt-abcdefgh"
    legacy.mkdir()
    (legacy / "keep").write_text("legacy", encoding="utf-8")
    (lease.target / "output").write_bytes(b"scratch")
    assert scratch.guard_scratch(env) == lease.target
    result = _finish(lease)
    assert result["state"] == "reclaimed"
    assert result["receipt"] is None
    assert lease.lock is None
    assert not lease.target.exists()
    # A reclaimed generation holds no custody, so its receipts are gone too.
    assert not lease.generation.exists()
    assert (legacy / "keep").read_text(encoding="utf-8") == "legacy"
    with pytest.raises(ValueError, match="scratch owner"):
        scratch.guard_scratch(env)


def test_guard_reclaims_readonly_hardlink_without_changing_external_source(
    tmp_path, readonly_file_source
):
    source, attributes = readonly_file_source
    before = attributes()
    lease, env = _lease(tmp_path)
    link = lease.target / "other-base_executable.exe"
    link.hardlink_to(source)
    assert scratch.guard_scratch(env) == lease.target

    def forbidden_chmod(*_args, **_kwargs):
        pytest.fail("guard cleanup must not mutate the borrowed source")

    with pytest.MonkeyPatch.context() as patch:
        patch.setattr(Path, "chmod", forbidden_chmod)
        result = _finish(lease)
    assert result["state"] == "reclaimed"
    assert result["retention"]["errors"] == []
    assert lease.lock is None
    assert not lease.target.exists()
    assert not lease.generation.exists()
    assert attributes() == before
    assert source.read_bytes() == b"external source must survive cleanup"


@pytest.mark.skipif(os.name != "posix", reason="POSIX directory write bits")
def test_guard_reclaims_sealed_published_image_without_changing_external_source(
    tmp_path, readonly_file_source
):
    from molt.python_environment_location import seal_environment_installation

    source, attributes = readonly_file_source
    before = attributes()
    lease, env = _lease(tmp_path)
    image = lease.target / "custody" / "environment"
    site = image / "lib" / "python3.12" / "site-packages"
    site.mkdir(parents=True)
    (site / "module.py").write_text("VALUE = 529\n", encoding="utf-8")
    (image / "bin").mkdir()
    (image / "bin" / "borrowed").hardlink_to(source)
    seal_environment_installation(image)
    # Independent oracle: an unowned delete cannot unlink inside a sealed tree.
    with pytest.raises(PermissionError):
        (site / "module.py").unlink()
    assert scratch.guard_scratch(env) == lease.target
    result = _finish(lease)
    assert result["state"] == "reclaimed", result
    assert result["retention"]["errors"] == []
    assert not lease.target.exists()
    assert not lease.generation.exists()
    assert attributes() == before
    assert source.read_bytes() == b"external source must survive cleanup"


def test_owned_temporary_directory_reclaims_readonly_hardlink(
    tmp_path, readonly_file_source
):
    source, attributes = readonly_file_source
    before = attributes()
    with scratch.OwnedTemporaryDirectory(dir=tmp_path) as owned:
        path = Path(owned)
        (path / "borrowed.exe").hardlink_to(source)
    assert not path.exists()
    assert attributes() == before
    assert source.read_bytes() == b"external source must survive cleanup"


@pytest.mark.parametrize(
    "field,invalid",
    [
        ("schema", "unknown-schema"),
        ("schema", None),
        ("token", "f" * 32),
        ("token", 17),
        ("generation", "other-generation"),
        ("generation", False),
        ("state", "unknown-state"),
        ("state", 17),
        ("state", []),
        ("state", {}),
        ("target_identity", "not-an-object"),
        ("target_identity", None),
    ],
)
def test_owner_rejection_identifies_field_without_granting_custody(
    tmp_path, field, invalid
):
    lease, env = _lease(tmp_path)
    owner_path = lease.generation / "owner.json"
    forged = dict(lease.owner, **{field: invalid})
    write_exact(owner_path, forged)
    try:
        with pytest.raises(ValueError, match="scratch owner mismatch") as caught:
            scratch.guard_scratch(env)
        message = str(caught.value)
        assert f"field={field}" in message
        assert f"observed_type={type(invalid).__name__}" in message
        assert env["MOLT_MEMORY_GUARD_TOKEN"] not in message
        if field == "token" and isinstance(invalid, str):
            assert invalid not in message
        assert _read(owner_path) == forged
        assert lease.target.is_dir()
        assert file_locks._file_lock_is_owned(lease.lock)
    finally:
        write_exact(owner_path, lease.owner)
        _finish(lease)


@pytest.mark.parametrize("invalid", [None, []])
def test_non_object_owner_rejection_reports_type_and_preserves_allocation(
    tmp_path, invalid
):
    lease, env = _lease(tmp_path)
    owner_path = lease.generation / "owner.json"
    write_exact(owner_path, invalid)
    try:
        with pytest.raises(ValueError, match="scratch owner mismatch") as caught:
            scratch.guard_scratch(env)
        message = str(caught.value)
        assert "field=owner" in message
        assert f"observed_type={type(invalid).__name__}" in message
        assert _read(owner_path) == invalid
        assert lease.target.is_dir()
        assert file_locks._file_lock_is_owned(lease.lock)
    finally:
        write_exact(owner_path, lease.owner)
        _finish(lease)


def test_owner_mismatch_diagnostic_bounds_and_escapes_untrusted_strings(tmp_path):
    lease, env = _lease(tmp_path)
    owner_path = lease.generation / "owner.json"
    invalid = "wrong\n" + "f" * 32 + "z" * 5000
    write_exact(owner_path, dict(lease.owner, schema=invalid))
    try:
        with pytest.raises(ValueError, match="scratch owner mismatch") as caught:
            scratch.guard_scratch(env)
        message = str(caught.value)
        assert "field=schema" in message
        assert "wrong\\n" in message
        assert "\n" not in message
        assert "f" * 32 not in message
        assert len(message) < 1500
        assert lease.target.is_dir()
    finally:
        write_exact(owner_path, lease.owner)
        _finish(lease)


def test_owner_token_diagnostic_accepts_escaped_invalid_unicode(tmp_path):
    lease, env = _lease(tmp_path)
    owner_path = lease.generation / "owner.json"
    # Child-written JSON may contain a string that cannot be UTF-8 encoded.
    forged = dict(lease.owner, token="\ud800")
    owner_path.write_text(json.dumps(forged, ensure_ascii=True), encoding="ascii")
    try:
        with pytest.raises(ValueError, match="field=token") as caught:
            scratch.guard_scratch(env)
        assert "observed_type=str" in str(caught.value)
        assert "observed=<redacted,length=1>" in str(caught.value)
        assert _read(owner_path) == forged
        assert lease.target.is_dir()
        assert file_locks._file_lock_is_owned(lease.lock)
    finally:
        write_exact(owner_path, lease.owner)
        _finish(lease)


def test_failure_payload_moves_inside_generation_and_retention_is_bounded(tmp_path):
    policy = scratch.ScratchRetention(count=1, bytes=5)
    first, _ = _lease(tmp_path)
    (first.target / "failure").write_bytes(b"1234")
    assert _finish(first, success=False, retention=policy)["state"] == "retained"
    assert (first.generation / "payload" / "failure").read_bytes() == b"1234"
    second, _ = _lease(tmp_path, 2)
    (second.target / "failure").write_bytes(b"5678")
    result = _finish(second, success=False, retention=policy)
    assert not (first.generation / "payload").exists()
    assert not first.generation.exists()
    assert result["retention"]["reclaimed"] == [str(first.generation)]
    assert result["retention"]["retained_bytes"] == 4
    assert (second.generation / "payload" / "failure").read_bytes() == b"5678"


def test_active_and_indeterminate_payloads_are_not_retention_candidates(tmp_path):
    active, _ = _lease(tmp_path)
    uncertain, _ = _lease(tmp_path, 2)
    try:
        result = _finish(uncertain, closed=False)
        assert result["state"] == "indeterminate"
        result = scratch.reclaim_terminal_scratch(
            active.generation.parent, retention=scratch.ScratchRetention(0, 0)
        )
        assert result["reclaimed"] == []
        assert result["errors"] == []
        assert result["protected_count"] == 0  # Only indexed terminal runs are scanned.
        assert active.target.is_dir() and uncertain.target.is_dir()
    finally:
        active.release()


def test_child_cannot_redirect_parent_cleanup_by_rewriting_owner(tmp_path):
    lease, _ = _lease(tmp_path)
    legacy = lease.target.parent / "pt-abcdefgh"
    legacy.mkdir()
    forged = dict(
        lease.owner, target=str(legacy), target_identity=scratch._identity(legacy)
    )
    write_exact(lease.generation / "owner.json", forged)
    with pytest.raises(ValueError, match="parent's allocation"):
        _finish(lease)
    assert lease.lock is None
    assert legacy.exists() and lease.target.exists()
    assert _read(lease.generation / "owner.json") == forged
    assert (
        "parent's allocation" in _read(lease.generation / "finish-error.json")["error"]
    )


def test_persisted_receipts_cannot_redirect_reclamation_to_legacy_sibling(tmp_path):
    lease, _ = _lease(tmp_path)
    _finish(lease, success=False)
    legacy = lease.target.parent / "pt-abcdefgh"
    legacy.mkdir()
    terminal = _read(lease.generation / "terminal.json")
    terminal.update(target=str(legacy), target_identity=scratch._identity(legacy))
    owner = _read(lease.generation / "owner.json")
    owner.update(
        target=str(legacy),
        target_identity=terminal["target_identity"],
        terminal_digest=canonical_json_sha256(terminal),
    )
    write_exact(lease.generation / "terminal.json", terminal)
    write_exact(lease.generation / "owner.json", owner)
    write_exact(
        scratch._index_path(lease.generation),
        {
            "schema": scratch.SCHEMA,
            "terminal_digest": owner["terminal_digest"],
        },
    )
    result = scratch.reclaim_terminal_scratch(
        lease.generation.parent, retention=scratch.ScratchRetention(0, 0)
    )
    assert any("inside its own generation" in error for error in result["errors"])
    assert legacy.exists()


def test_measurement_failure_preserves_payload_and_records_indeterminate(
    tmp_path, monkeypatch
):
    lease, _ = _lease(tmp_path)

    def fail_measurement(_):
        raise OSError("fixture measurement failure")

    monkeypatch.setattr(scratch, "tree_bytes", fail_measurement)
    with pytest.raises(OSError, match="measurement failure"):
        _finish(lease, success=False)
    assert lease.target.is_dir() and lease.lock is None
    assert _read(lease.generation / "owner.json")["state"] == "indeterminate"
    assert _read(lease.generation / "finish-error.json")["closed"] is True


def test_pending_index_does_not_scan_historical_receipts(tmp_path, monkeypatch):
    old, _ = _lease(tmp_path)
    _finish(old)
    recent, _ = _lease(tmp_path, 2)
    _finish(recent, success=False)
    original = scratch._owner

    def read_owner(generation):
        assert generation != old.generation, "history must not enter pending discovery"
        return original(generation)

    monkeypatch.setattr(scratch, "_owner", read_owner)
    result = scratch.reclaim_terminal_scratch(recent.generation.parent)
    assert result["retained_count"] == 1 and not result["errors"]


def test_pending_publication_exposes_only_committed_locked_generations(
    tmp_path, monkeypatch
):
    prior, _ = _lease(tmp_path)
    _finish(prior, success=False)
    active, _ = _lease(tmp_path, 2)
    root = active.generation.parent
    write = scratch.write_exact
    move = scratch.namespace_move_exclusive
    checkpoints = []
    discovered = []

    def write_private(path, value, **kwargs):
        write(path, value, **kwargs)
        if path == active.generation / "pending.json":
            result = scratch.reclaim_terminal_scratch(root)
            assert result["errors"] == []
            assert result["retained_count"] == 1
            assert result["protected_count"] == 0
            checkpoints.append("private-stage")

    def publish_index(staged, destination):
        move(staged, destination)
        if destination != scratch._index_path(active.generation):
            return  # The reclaimed generation's own removal.
        assert staged == active.generation / "pending.json"
        discovered.extend(destination.parent.iterdir())
        result = scratch.reclaim_terminal_scratch(root)
        assert result["errors"] == []
        assert result["retained_count"] == 1
        assert result["protected_count"] == 1
        checkpoints.append("published-with-owner-lock")

    monkeypatch.setattr(scratch, "write_exact", write_private)
    monkeypatch.setattr(scratch, "namespace_move_exclusive", publish_index)
    result = _finish(active)
    assert result["retention"]["errors"] == []
    assert checkpoints == ["private-stage", "published-with-owner-lock"]
    iterdir = Path.iterdir
    monkeypatch.setattr(
        Path,
        "iterdir",
        lambda path: iter(discovered) if path == root / "pending" else iterdir(path),
    )
    result = scratch.reclaim_terminal_scratch(root)
    assert result["errors"] == []
    assert result["protected_count"] == 0
    assert result["retained_count"] == 1


def test_pending_sweep_treats_empty_locked_generation_as_busy_not_io_failure(tmp_path):
    lease, _ = _lease(tmp_path)
    _finish(lease, success=False)
    lock_path = lease.generation / "lock"
    with lock_path.open("r+b", buffering=0) as holder:
        assert file_locks._try_lock_file_handle(holder)
        try:
            # Reproduce the historical advisory-PID truncate window without
            # changing scratch authority or teaching the sweep to ignore I/O.
            holder.truncate(0)
            result = scratch.reclaim_terminal_scratch(
                lease.generation.parent, retention=scratch.ScratchRetention(0, 0)
            )
            assert result["errors"] == []
            assert result["protected_count"] == 1
            assert result["reclaimed"] == []
            assert lock_path.stat().st_size == 0
            assert (lease.generation / "payload").is_dir()
        finally:
            file_locks._unlock_file_handle(holder)
    result = scratch.reclaim_terminal_scratch(
        lease.generation.parent, retention=scratch.ScratchRetention(0, 0)
    )
    assert result["errors"] == []
    assert result["reclaimed"] == [str(lease.generation)]
    assert not (lease.generation / "payload").exists()


@pytest.mark.parametrize("terminal_reclaimed", [False, True])
def test_pending_snapshot_removal_requires_locked_terminal_authority(
    tmp_path, monkeypatch, terminal_reclaimed
):
    lease, _ = _lease(tmp_path)
    _finish(lease, success=False)
    pending = scratch._index_path(lease.generation).parent
    iterdir = Path.iterdir

    def snapshot_then_remove(path):
        entries = tuple(iterdir(path))
        if path == pending:
            with scratch._locked(lease.generation):
                if terminal_reclaimed:
                    scratch._reclaim_locked(
                        lease.generation, scratch._owner(lease.generation)
                    )
                else:
                    scratch._drop_index(lease.generation)
        return iter(entries)

    monkeypatch.setattr(Path, "iterdir", snapshot_then_remove)
    result = scratch.reclaim_terminal_scratch(lease.generation.parent)
    assert result["retained_count"] == 0
    assert bool(result["errors"]) is not terminal_reclaimed
    if not terminal_reclaimed:
        assert "scratch pending index is unavailable" in result["errors"][0]
        assert (lease.generation / "payload").is_dir()


def test_retention_budget_revalidates_candidate_after_discovery(tmp_path, monkeypatch):
    lease, _ = _lease(tmp_path)
    _finish(lease, success=False)
    locked = scratch._locked
    visits = 0

    @contextmanager
    def reclaim_before_budget(generation):
        nonlocal visits
        with locked(generation):
            visits += 1
            if visits == 2:
                scratch._reclaim_locked(generation, scratch._owner(generation))
            yield

    monkeypatch.setattr(scratch, "_locked", reclaim_before_budget)
    result = scratch.reclaim_terminal_scratch(lease.generation.parent)
    assert visits == 2
    assert result["errors"] == []
    assert result["retained_count"] == result["retained_bytes"] == 0


@pytest.mark.parametrize(
    "name",
    ["unexpected.tmp", ".molt-write-" + "0" * 16 + "-" + "1" * 32 + ".tmp", "bad.json"],
)
def test_unowned_pending_entries_remain_fail_closed(tmp_path, name):
    lease, _ = _lease(tmp_path)
    _finish(lease, success=False)
    invalid = scratch._index_path(lease.generation).parent / name
    invalid.write_text("{}", encoding="utf-8")
    result = scratch.reclaim_terminal_scratch(lease.generation.parent)
    assert result["errors"] == [f"{invalid}: invalid scratch pending entry"]
    assert invalid.read_text(encoding="utf-8") == "{}"
    assert (lease.generation / "payload").is_dir()


@pytest.mark.parametrize("leaf", ["owner", "index", "terminal"])
def test_corrupt_retained_authority_is_not_excused_as_discovery_race(tmp_path, leaf):
    lease, _ = _lease(tmp_path)
    _finish(lease, success=False)
    path = (
        scratch._index_path(lease.generation)
        if leaf == "index"
        else lease.generation / (leaf + ".json")
    )
    write_exact(path, {"schema": "malformed"})
    result = scratch.reclaim_terminal_scratch(lease.generation.parent)
    assert result["errors"]
    assert result["retained_count"] == 0
    assert (lease.generation / "payload").is_dir()


def test_retirement_write_failure_has_pending_discovery_and_preserves_source(
    tmp_path, monkeypatch
):
    lease, _ = _lease(tmp_path)
    original = scratch.write_exact

    def fail_retiring(path, value, **kwargs):
        if path == lease.generation / "owner.json" and value.get("state") == "retiring":
            assert scratch._index_path(lease.generation).is_file()
            raise OSError("fixture retirement publication failure")
        original(path, value, **kwargs)

    monkeypatch.setattr(scratch, "write_exact", fail_retiring)
    with pytest.raises(OSError, match="retirement publication"):
        _finish(lease)
    assert lease.target.is_dir() and lease.lock is None
    assert _read(lease.generation / "owner.json")["state"] == "indeterminate"
    result = scratch.reclaim_terminal_scratch(lease.generation.parent)
    assert result["errors"] and not result["reclaimed"]
    assert lease.target.is_dir()


@pytest.mark.parametrize("owner_state", ["leased", "indeterminate"])
def test_uncommitted_retirement_is_reported_once_and_preserves_source(
    tmp_path, monkeypatch, owner_state
):
    lease, _ = _lease(tmp_path)
    leased_owner = dict(lease.owner)
    original = scratch.write_exact

    def fail_retiring(path, value, **kwargs):
        if path == lease.generation / "owner.json" and value.get("state") == "retiring":
            raise OSError("fixture owner commit interrupted")
        original(path, value, **kwargs)

    monkeypatch.setattr(scratch, "write_exact", fail_retiring)
    with pytest.raises(OSError, match="owner commit interrupted"):
        _finish(lease)
    monkeypatch.setattr(scratch, "write_exact", original)
    if owner_state == "leased":
        # A killed guard never reaches its indeterminate failure receipt.
        write_exact(lease.generation / "owner.json", leased_owner)
    assert scratch._index_path(lease.generation).is_file()

    first = scratch.reclaim_terminal_scratch(lease.generation.parent)
    assert first["errors"] == [
        f"{lease.generation}: interrupted before the terminal owner commit; "
        "payload preserved without retry"
    ]
    owner = _read(lease.generation / "owner.json")
    assert owner["state"] == "blocked"
    assert owner["terminal_digest"] == canonical_json_sha256(
        _read(lease.generation / "terminal.json")
    )
    assert not scratch._index_path(lease.generation).exists()
    assert lease.target.is_dir()

    second = scratch.reclaim_terminal_scratch(lease.generation.parent)
    assert second["errors"] == []
    assert lease.target.is_dir()


def test_uncommitted_retirement_with_foreign_terminal_stays_fail_closed(tmp_path):
    lease, _ = _lease(tmp_path)
    leased_owner = dict(lease.owner)
    _finish(lease, success=False)
    write_exact(lease.generation / "owner.json", leased_owner)
    terminal = _read(lease.generation / "terminal.json")
    write_exact(lease.generation / "terminal.json", dict(terminal, token="f" * 32))
    for _ in range(2):
        result = scratch.reclaim_terminal_scratch(lease.generation.parent)
        assert result["errors"]
        assert scratch._index_path(lease.generation).is_file()
    assert _read(lease.generation / "owner.json") == leased_owner


def test_missing_retained_payload_is_reported_not_counted_as_retained(tmp_path):
    lease, _ = _lease(tmp_path)
    _finish(lease, success=False)
    shutil.rmtree((lease.generation / "payload"))
    result = scratch.reclaim_terminal_scratch(lease.generation.parent)
    assert result["errors"] and result["retained_count"] == 0


@pytest.mark.parametrize("present", [False, True])
def test_interrupted_retirement_requires_nested_payload_identity(tmp_path, present):
    lease, _ = _lease(tmp_path)
    _finish(lease, success=False)
    payload = lease.generation / "payload"
    if not present:
        shutil.rmtree(payload)
    owner = _read(lease.generation / "owner.json")
    write_exact(lease.generation / "owner.json", dict(owner, state="retiring"))
    scratch.reclaim_terminal_scratch(lease.generation.parent)
    assert _read(lease.generation / "owner.json")["state"] == (
        "retained" if present else "blocked"
    )


def test_interrupted_success_reclaims_without_consuming_failure_budget(tmp_path):
    lease, _ = _lease(tmp_path)
    _finish(lease, success=False)
    terminal = _read(lease.generation / "terminal.json")
    terminal["success"] = True
    owner = _read(lease.generation / "owner.json")
    owner.update(state="retiring", terminal_digest=canonical_json_sha256(terminal))
    write_exact(lease.generation / "terminal.json", terminal)
    write_exact(lease.generation / "owner.json", owner)
    write_exact(
        scratch._index_path(lease.generation),
        {
            "schema": scratch.SCHEMA,
            "terminal_digest": owner["terminal_digest"],
        },
    )
    result = scratch.reclaim_terminal_scratch(lease.generation.parent)
    assert result["reclaimed"] == [str(lease.generation)]
    assert result["retained_count"] == 0


@pytest.mark.parametrize("leaf", ["pending-entry", "generation"])
def test_indirect_pending_entries_fail_closed_with_diagnostics(
    tmp_path, monkeypatch, leaf
):
    from molt import file_publication

    lease, _ = _lease(tmp_path)
    _finish(lease, success=False)
    selected = (
        scratch._index_path(lease.generation)
        if leaf == "pending-entry"
        else lease.generation
    )
    original = file_publication.is_link_like
    monkeypatch.setattr(
        file_publication,
        "is_link_like",
        lambda path: path == selected or original(path),
    )
    result = scratch.reclaim_terminal_scratch(lease.generation.parent)
    assert result["errors"] and (lease.generation / "payload").is_dir()


@pytest.mark.parametrize("present", [False, True])
def test_interrupted_reclamation_repairs_receipt_but_never_retries_payload(
    tmp_path, monkeypatch, present
):
    lease, _ = _lease(tmp_path)
    _finish(lease, success=False)
    target = lease.generation / "payload"
    if not present:
        shutil.rmtree(target)
    owner = _read(lease.generation / "owner.json")
    write_exact(lease.generation / "owner.json", dict(owner, state="reclaiming"))
    delete_path = scratch.delete_path

    def delete_receipts_only(path, *, owning_root=None):
        if path == target:
            pytest.fail("must not retry")
        return delete_path(path, owning_root=owning_root)

    monkeypatch.setattr(scratch, "delete_path", delete_receipts_only)
    scratch.reclaim_terminal_scratch(
        lease.generation.parent, retention=scratch.ScratchRetention(0, 0)
    )
    if present:
        assert _read(lease.generation / "owner.json")["state"] == "blocked"
        assert target.is_dir()
    else:
        # The repaired receipt says reclaimed, which then removes the receipts.
        assert not lease.generation.exists()


def test_delete_failure_is_durable_and_not_silenced(tmp_path, monkeypatch):
    lease, _ = _lease(tmp_path)
    monkeypatch.setattr(
        scratch,
        "delete_path",
        lambda path, *, owning_root=None: (False, "fixture sharing violation"),
    )
    result = _finish(lease)
    assert result["state"] == "blocked"
    assert "sharing violation" in _read(lease.generation / "owner.json")["error"]
    assert (lease.generation / "payload").exists()


@pytest.mark.parametrize("budget", [3, 4])
def test_failed_payload_bytes_decide_retention(tmp_path, budget):
    lease, _ = _lease(tmp_path)
    (lease.target / "output").write_bytes(b"1234")
    result = _finish(
        lease, success=False, retention=scratch.ScratchRetention(3, budget)
    )
    if budget < 4:
        assert result["state"] == "reclaimed"
        assert not lease.generation.exists()
    else:
        assert result["state"] == "retained"
        assert _read(lease.generation / "terminal.json")["retained_bytes"] == 4
        assert (lease.generation / "payload" / "output").read_bytes() == b"1234"


def test_existing_generation_is_never_adopted_by_a_new_parent(tmp_path):
    lease, env = _lease(tmp_path)
    try:
        with pytest.raises(FileExistsError):
            scratch.acquire_guard_scratch(tmp_path, env)
    finally:
        _finish(lease)


@pytest.mark.parametrize("leaf", ["owner.json", "terminal.json", "lock"])
def test_link_like_custody_leaves_are_not_followed(tmp_path, monkeypatch, leaf):
    from molt import file_publication

    lease, _ = _lease(tmp_path)
    _finish(lease, success=False)
    selected = lease.generation / leaf
    original = file_publication.is_link_like
    monkeypatch.setattr(
        file_publication,
        "is_link_like",
        lambda path: path == selected or original(path),
    )
    result = scratch.reclaim_terminal_scratch(
        lease.generation.parent, retention=scratch.ScratchRetention(0, 0)
    )
    assert result["errors"]
    assert (lease.generation / "payload").exists()


@pytest.mark.parametrize(
    "prefix", ["../escape", "/absolute", "..\\escape", "C:\\outside", "", "a" * 49]
)
def test_guarded_helper_rejects_non_basename_prefixes(tmp_path, prefix):
    lease, env = _lease(tmp_path)
    try:
        with pytest.raises(ValueError, match="basename"):
            scratch.new_guarded_directory(env, prefix=prefix)
    finally:
        _finish(lease)


def test_scratch_allocator_uses_host_directory_custody_mode(monkeypatch, tmp_path):
    modes = []
    mkdir = Path.mkdir

    def observe(path, mode=0o777, parents=False, exist_ok=False):
        modes.append(mode)
        return mkdir(path, mode=mode, parents=parents, exist_ok=exist_ok)

    monkeypatch.setattr(Path, "mkdir", observe)
    path = scratch.new_temporary_directory(tmp_path, prefix="compile-")
    assert modes == [0o755 if os.name == "nt" else 0o700]
    assert path.parent == tmp_path
    assert list(path.iterdir()) == []


def test_scratch_allocator_does_not_reuse_a_colliding_directory(monkeypatch, tmp_path):
    existing = tmp_path / "compile-aaaaaaaa"
    existing.mkdir()
    marker = existing / "owner.txt"
    marker.write_text("other allocation", encoding="utf-8")
    characters = iter("aaaaaaaa" + "bbbbbbbb")
    monkeypatch.setattr(scratch.secrets, "choice", lambda _alphabet: next(characters))
    allocated = scratch.new_temporary_directory(tmp_path, prefix="compile-")
    assert allocated == tmp_path / "compile-bbbbbbbb"
    assert marker.read_text(encoding="utf-8") == "other allocation"
    assert list(allocated.iterdir()) == []


def test_owned_temporary_directory_matches_scoped_cleanup_contract(tmp_path):
    with scratch.OwnedTemporaryDirectory(dir=tmp_path, prefix="compiler-") as name:
        path = Path(name)
        assert path.parent == tmp_path
        payload = path / "bytes.bin"
        payload.write_bytes(b"owned bytes")
        payload.chmod(0o444)
        assert payload.read_bytes() == b"owned bytes"
    assert not path.exists()


def test_owned_temporary_directory_cleanup_is_idempotent(tmp_path):
    directory = scratch.OwnedTemporaryDirectory(dir=tmp_path)
    directory.cleanup()
    directory.cleanup()
    assert not Path(directory.name).exists()


def test_owned_temporary_directory_does_not_delete_a_replaced_allocation(tmp_path):
    directory = scratch.OwnedTemporaryDirectory(dir=tmp_path)
    original = Path(directory.name)
    retained = original.with_name("retained-original")
    original.rename(retained)
    original.mkdir()
    marker = original / "other-owner.txt"
    marker.write_text("retain", encoding="utf-8")
    with pytest.raises(ValueError, match="allocation changed"):
        directory.cleanup()
    assert marker.read_text(encoding="utf-8") == "retain"
    assert retained.exists()


def test_helper_subdirectories_are_owned_by_terminal_guard_not_context_age(tmp_path):
    lease, env = _lease(tmp_path)
    first = scratch.new_guarded_directory(env, prefix="compile-")
    second = scratch.new_guarded_directory(env, prefix="compile-")
    assert first.parent == second.parent == lease.target
    assert first != second
    _finish(lease)
    assert not first.exists() and not second.exists()


def test_proof_helpers_survive_nested_guard_retirement(tmp_path):
    lease, env = _lease(tmp_path)
    proof_root = tmp_path / "proof-output" / "scratch"
    proof_root.mkdir(parents=True)
    env["MOLT_PROOF_SCRATCH_ROOT"] = str(proof_root)
    helper = scratch.new_guarded_directory(env, prefix="compile-")
    payload = helper / "compiled-image"
    payload.write_bytes(b"retained for the queue owner")
    assert helper.parent == proof_root
    _finish(lease)
    assert not lease.target.exists()
    assert payload.read_bytes() == b"retained for the queue owner"


@pytest.mark.parametrize("count,bytes_", [(-1, 1), (1, -1), (True, 0), (1, 1.5)])
def test_retention_limits_are_exact_nonnegative_integers(count, bytes_):
    with pytest.raises(ValueError):
        scratch.ScratchRetention(count, bytes_)


@pytest.mark.parametrize(
    "invalidity", ["released", "closed", "wrong-process", "different-generation"]
)
def test_terminal_publication_requires_actual_live_parent_lock(
    tmp_path, monkeypatch, invalidity
):
    lease, _ = _lease(tmp_path)
    handle = lease.lock
    assert handle is not None
    try:
        if invalidity == "released":
            file_locks._release_file_lock(handle)
        elif invalidity == "closed":
            handle.file.close()
        elif invalidity == "wrong-process":
            install_module_view(
                monkeypatch,
                "os",
                os,
                file_locks,
                getpid=lambda: handle.owner_process_id + 1,
            )
        else:
            original_key = handle.registry_key
            handle.registry_key = original_key + ".other"
        with pytest.raises(ValueError, match="live ownership"):
            _finish(lease)
        assert lease.target.exists()
        assert not (lease.generation / "terminal.json").exists()
    finally:
        monkeypatch.undo()
        if invalidity == "different-generation":
            handle.registry_key = original_key
        lease.release()


def test_terminal_publication_pins_custody_against_postcheck_transfer(
    tmp_path, monkeypatch
):
    import threading

    lease, _ = _lease(tmp_path)
    handle = lease.lock
    reached = threading.Event()
    attempted = threading.Event()
    transferred = threading.Event()
    contender = []
    errors = []
    original = scratch._publish_index

    def transfer():
        try:
            assert reached.wait(5)
            attempted.set()
            file_locks._release_file_lock(handle)
            contender.append(
                file_locks._try_acquire_file_lock(lease.generation / "lock")
            )
            transferred.set()
        except BaseException as exc:
            errors.append(exc)

    def publish(*args):
        reached.set()
        assert attempted.wait(5)
        assert not transferred.wait(0.05)
        assert file_locks._file_lock_is_owned(handle)
        return original(*args)

    remove = scratch._remove_reclaimed_generation

    def remove_after_transfer(generation):
        # The finisher removes its reclaimed receipts once its lock is free.
        # Order that after the transfer so the contender's lock is certain.
        assert transferred.wait(5)
        return remove(generation)

    monkeypatch.setattr(scratch, "_publish_index", publish)
    monkeypatch.setattr(scratch, "_remove_reclaimed_generation", remove_after_transfer)
    thread = threading.Thread(target=transfer)
    thread.start()
    try:
        result = _finish(lease)
        assert result["state"] == "reclaimed"
        assert transferred.wait(5)
        assert contender[0] is not None
        assert not file_locks._file_lock_is_owned(handle)
        assert file_locks._file_lock_is_owned(contender[0])
        if os.name == "nt":
            # Windows cannot move a directory while its lock file is open:
            # the removal waits for the next sweep, which keeps the index.
            assert _read(lease.generation / "owner.json")["state"] == "reclaimed"
            assert result["retention"]["deferred"]
            assert scratch._index_path(lease.generation).is_file()
        else:
            # Reclaimed is final, so the held lock does not keep the receipts.
            assert not lease.generation.exists()
            assert not scratch._index_path(lease.generation).exists()
    finally:
        reached.set()
        thread.join(5)
        lease.release()
        for acquired in contender:
            if acquired is not None:
                file_locks._release_file_lock(acquired)
    assert not thread.is_alive()
    assert not errors


def test_terminal_callback_cannot_revoke_its_own_custody(tmp_path, monkeypatch):
    lease, _ = _lease(tmp_path)
    handle = lease.lock
    original = scratch._publish_index

    def publish(*args):
        with pytest.raises(RuntimeError, match="owned operation"):
            lease.release()
        assert lease.lock is handle
        assert file_locks._file_lock_is_owned(handle)
        return original(*args)

    monkeypatch.setattr(scratch, "_publish_index", publish)
    assert _finish(lease)["state"] == "reclaimed"
    assert lease.lock is None
    assert not handle.operation_owners


def test_terminal_callback_failure_releases_pin_and_preserves_payload(
    tmp_path, monkeypatch
):
    lease, _ = _lease(tmp_path)
    handle = lease.lock

    def fail(*args):
        assert file_locks._file_lock_is_owned(handle)
        raise RuntimeError("forced pinned publication failure")

    monkeypatch.setattr(scratch, "_publish_index", fail)
    with pytest.raises(RuntimeError, match="forced pinned publication failure"):
        _finish(lease)
    assert lease.target.exists()
    assert lease.lock is None
    assert not handle.operation_owners
    assert not file_locks._file_lock_is_owned(handle)


@pytest.mark.parametrize("explicit_disk", [False, True])
def test_allocation_refuses_a_run_whose_scratch_budget_is_not_free(
    tmp_path, explicit_disk
):
    state = tmp_path / "tmp" / "memory_guard"
    env = {
        "MOLT_MEMORY_GUARD_STATE_ROOT": str(state),
        "MOLT_MEMORY_GUARD_TOKEN": f"{7:032x}",
        "MOLT_MEMORY_GUARD_MARKER": str(state / "active" / "guard-7.json"),
        # One exbibyte: no host volume has it free.
        "MOLT_SCRATCH_BUDGET_GB": str(1024**3),
    }
    repo = tmp_path / "source"
    if explicit_disk:
        selected = tmp_path / "selected"
        selected.mkdir()
        env["MOLT_SCRATCH_STORAGE"] = f"disk:{selected}"
    allocation_root = scratch.scratch_root(repo, env)

    with pytest.raises(ValueError, match="scratch capacity admission rejected"):
        scratch.acquire_guard_scratch(repo, env)
    # Admission runs before allocation: nothing was created.
    assert not allocation_root.exists()


def test_guard_scratch_follows_the_selected_scratch_storage(tmp_path):
    ram = tmp_path / "ram"
    ram.mkdir()
    family = tmp_path / "Molt"
    lane = family / "worktrees" / "lane"
    lane.mkdir(parents=True)
    env = {"MOLT_SCRATCH_STORAGE": str(ram)}

    root = scratch.scratch_root(lane, env)

    assert root.parent.parent == ram.resolve()
    assert root.name == "gs"


@pytest.mark.parametrize("prefix", ["disk:", ""])
def test_explicit_data_storage_preserves_control_state_and_lease_lifecycle(
    tmp_path, prefix
):
    selected = tmp_path / "selected"
    selected.mkdir()
    repo = tmp_path / "source"
    repo.mkdir()
    lease, env = _lease(repo, storage=prefix + str(selected))
    try:
        assert selected in lease.target.parents
        assert selected in lease.generation.parents
        assert lease.owner["guard_marker"] == str(
            repo / "tmp" / "memory_guard" / "active" / "guard-1.json"
        )
        assert scratch.guard_scratch(env) == lease.target
        assert not (repo / "tmp" / "gs").exists()
    finally:
        result = _finish(lease)
    assert result["state"] == "reclaimed"
    assert not lease.target.exists()


# --- dead-guard resolution and receipt removal ------------------------------


_CLOSURE = {"schema": "molt.guard-scratch-closure.v1", "closed": True}


def _marker_of(env):
    return Path(env["MOLT_MEMORY_GUARD_MARKER"])


def test_scratch_generation_follows_receipt_or_lease_geometry(tmp_path):
    lease, env = _lease(tmp_path)
    try:
        token = env["MOLT_MEMORY_GUARD_TOKEN"]
        leased = {"state": "leased", "target": str(lease.target)}
        assert scratch.scratch_generation(token, leased) == lease.generation
        receipt = {
            "state": "indeterminate",
            "receipt": str(lease.generation / "owner.json"),
        }
        assert scratch.scratch_generation(token, receipt) == lease.generation
        assert scratch.scratch_generation(token, {"state": "reclaimed"}) is None
    finally:
        lease.release()


def test_a_long_runs_closure_keeps_its_terminal_readable(tmp_path):
    """A guard that closed hundreds of nested generations still leaves a
    terminal the receipt reader accepts and retention reclaims. Copying the
    whole closure made a 69 KiB terminal on 2026-10-22, past the 64 KiB
    reader, and every later sweep on the host failed on it."""
    lease, env = _lease(tmp_path)
    lease.release()
    generations = [
        {"disposition": "closed", "path": f"/guard/retired/guard-{n}-{'0' * 32}.json"}
        for n in range(2000)
    ]
    closure = {**_CLOSURE, "guard_generations": generations}
    outcome = scratch.resolve_guard_scratch(
        lease.generation, guard_marker=_marker_of(env), closure=closure
    )
    assert (outcome["state"], outcome["resolved"]) == ("retained", True)
    terminal = _read(lease.generation / "terminal.json")
    assert terminal["closure"]["closed"] is True
    assert terminal["closure"]["guard_generations"]["count"] == 2000
    result = scratch.reclaim_terminal_scratch(
        lease.generation.parent, retention=scratch.ScratchRetention(0, 0)
    )
    assert result["reclaimed"] == [str(lease.generation)]


def test_live_lease_and_unproven_closure_stay_unresolved(tmp_path):
    lease, env = _lease(tmp_path)
    marker = _marker_of(env)
    busy = scratch.resolve_guard_scratch(
        lease.generation, guard_marker=marker, closure=_CLOSURE
    )
    assert (busy["state"], busy["resolved"]) == ("busy", False)
    lease.release()
    unproven = scratch.resolve_guard_scratch(
        lease.generation, guard_marker=marker, closure=None
    )
    assert (unproven["state"], unproven["resolved"]) == ("leased", False)
    assert _read(lease.generation / "owner.json")["state"] == "leased"
    assert lease.target.is_dir()


def test_proven_dead_lease_takes_the_failure_retention_path(tmp_path):
    lease, env = _lease(tmp_path)
    (lease.target / "output").write_bytes(b"1234")
    lease.release()
    outcome = scratch.resolve_guard_scratch(
        lease.generation, guard_marker=_marker_of(env), closure=_CLOSURE
    )
    assert (outcome["state"], outcome["resolved"]) == ("retained", True)
    assert (lease.generation / "payload" / "output").read_bytes() == b"1234"
    terminal = _read(lease.generation / "terminal.json")
    assert terminal["closure"] == _CLOSURE and terminal["finished_ns"] == 0
    # The existing retention bound reclaims it like any other failure.
    result = scratch.reclaim_terminal_scratch(
        lease.generation.parent, retention=scratch.ScratchRetention(0, 0)
    )
    assert result["reclaimed"] == [str(lease.generation)]
    assert not lease.generation.exists()


def test_adopted_payloads_sort_behind_real_failures(tmp_path):
    real, _ = _lease(tmp_path)
    (real.target / "failure").write_bytes(b"1")
    assert _finish(real, success=False)["state"] == "retained"
    dead, env = _lease(tmp_path, 2)
    (dead.target / "failure").write_bytes(b"2")
    dead.release()
    scratch.resolve_guard_scratch(
        dead.generation, guard_marker=_marker_of(env), closure=_CLOSURE
    )
    result = scratch.reclaim_terminal_scratch(
        real.generation.parent, retention=scratch.ScratchRetention(1, 1024)
    )
    assert result["reclaimed"] == [str(dead.generation)]
    assert (real.generation / "payload" / "failure").read_bytes() == b"1"


def test_dead_lease_without_its_payload_resolves_and_is_removed(tmp_path):
    lease, env = _lease(tmp_path)
    finished = _finish(lease, closed=False)
    assert finished["state"] == "indeterminate"
    shutil.rmtree(lease.target)
    outcome = scratch.resolve_guard_scratch(
        lease.generation, guard_marker=_marker_of(env), closure=_CLOSURE
    )
    assert outcome == {
        "generation": str(lease.generation),
        "state": "reclaimed",
        "resolved": True,
        "deferred": None,
    }
    assert not lease.generation.exists()
    again = scratch.resolve_guard_scratch(
        lease.generation, guard_marker=_marker_of(env), closure=_CLOSURE
    )
    assert (again["state"], again["resolved"]) == ("absent", True)


@pytest.mark.parametrize("inode_reused", [False, True])
def test_dead_lease_whose_target_is_another_directory_stays_blocked(
    tmp_path, monkeypatch, inode_reused
):
    lease, env = _lease(tmp_path)
    lease.release()
    original = scratch._identity(lease.target)
    shutil.rmtree(lease.target)
    lease.target.mkdir()
    (lease.target / "other").write_text("keep", encoding="utf-8")
    if inode_reused:
        # Linux hands a freed inode number to the next directory.
        identity = scratch._identity
        monkeypatch.setattr(
            scratch,
            "_identity",
            lambda path: original if path == lease.target else identity(path),
        )
    outcome = scratch.resolve_guard_scratch(
        lease.generation, guard_marker=_marker_of(env), closure=_CLOSURE
    )
    assert (outcome["state"], outcome["resolved"]) == ("blocked", True)
    assert (lease.target / "other").read_text(encoding="utf-8") == "keep"
    assert "another directory" in _read(lease.generation / "owner.json")["error"]


def test_target_receipt_is_custody_not_payload(tmp_path):
    lease, env = _lease(tmp_path)
    receipt = _read(lease.target / ".molt-scratch-target.json")
    assert receipt == {"schema": scratch.SCHEMA, "nonce": lease.owner["target_receipt"]}
    (lease.target / "output").write_bytes(b"1234")
    lease.release()
    outcome = scratch.resolve_guard_scratch(
        lease.generation, guard_marker=_marker_of(env), closure=_CLOSURE
    )
    assert outcome["state"] == "retained"
    assert _read(lease.generation / "terminal.json")["retained_bytes"] == 4


@pytest.mark.parametrize("owner_names_receipt", [True, False])
def test_a_target_without_its_receipt_is_adopted_only_by_an_older_owner(
    tmp_path, owner_names_receipt
):
    lease, env = _lease(tmp_path)
    lease.release()
    (lease.target / ".molt-scratch-target.json").unlink()
    if not owner_names_receipt:
        # Owners written before receipts existed record only file identity.
        owner = {k: v for k, v in lease.owner.items() if k != "target_receipt"}
        write_exact(lease.generation / "owner.json", owner)
    outcome = scratch.resolve_guard_scratch(
        lease.generation, guard_marker=_marker_of(env), closure=_CLOSURE
    )
    assert outcome["state"] == ("blocked" if owner_names_receipt else "retained")


def test_consumer_finds_its_lease_whatever_state_root_it_sees(tmp_path):
    """The HF-163 CI split: a process redirects the guards it starts."""
    lease, env = _lease(tmp_path)
    try:
        moved = dict(env, MOLT_MEMORY_GUARD_STATE_ROOT=str(tmp_path / "elsewhere"))
        assert scratch.guard_scratch(moved) == lease.target
        helper = scratch.new_guarded_directory(moved, prefix="helper-")
        assert helper.parent == lease.target
        missing = {k: v for k, v in env.items() if k != scratch.SCRATCH_ENV}
        with pytest.raises(ValueError, match="active parent's allocation"):
            scratch.guard_scratch(missing)
    finally:
        _finish(lease)


@pytest.mark.parametrize("reader", ["sweep", "resolve"])
@pytest.mark.parametrize(
    "move_at",
    [
        "before-lock",
        pytest.param(
            "owner",
            marks=pytest.mark.skipif(
                os.name == "nt",
                reason="Win32 holds the namespace through the open lock",
            ),
        ),
        pytest.param(
            "terminal",
            marks=pytest.mark.skipif(
                os.name == "nt",
                reason="Win32 holds the namespace through the open lock",
            ),
        ),
    ],
)
def test_sweep_never_revives_a_generation_moved_away_under_it(
    tmp_path, monkeypatch, reader, move_at
):
    """Actual final namespace removal can race a POSIX locked receipt reader."""
    lease, env = _lease(tmp_path)
    _finish(lease, success=False)
    with scratch._locked(lease.generation):
        assert (
            scratch._reclaim_locked(lease.generation, scratch._owner(lease.generation))[
                "state"
            ]
            == "reclaimed"
        )
    removing = lease.generation.parent / "removing"
    removing.mkdir()
    identity = scratch._identity
    read = scratch.read_exact
    moved = []

    def move():
        moved.append(lease.generation)
        os.rename(lease.generation, removing / lease.generation.name)

    def concurrent_identity(path):
        result = identity(path)
        if path == lease.generation and not moved and move_at == "before-lock":
            move()
        return result

    def concurrent_read(path, **kwargs):
        if path == lease.generation / f"{move_at}.json" and not moved:
            move()
        return read(path, **kwargs)

    monkeypatch.setattr(scratch, "_identity", concurrent_identity)
    monkeypatch.setattr(scratch, "read_exact", concurrent_read)
    if reader == "sweep":
        result = scratch.reclaim_terminal_scratch(lease.generation.parent)
        assert result["errors"] == []
    else:
        result = scratch.resolve_guard_scratch(
            lease.generation,
            guard_marker=Path(env["MOLT_MEMORY_GUARD_MARKER"]),
            closure=None,
        )
        assert result == {
            "generation": str(lease.generation),
            "state": "absent",
            "resolved": True,
        }
        assert scratch.reclaim_terminal_scratch(lease.generation.parent)["errors"] == []
    assert moved
    # Neither reader revives custody or reports completed removal as a crash.
    assert not lease.generation.exists()
    assert not scratch._index_path(lease.generation).exists()


def test_index_of_a_removed_generation_is_resolved_not_an_error(tmp_path):
    lease, _ = _lease(tmp_path)
    _finish(lease, success=False)
    index = scratch._index_path(lease.generation)
    shutil.rmtree(lease.generation)
    result = scratch.reclaim_terminal_scratch(lease.generation.parent)
    assert result["errors"] == []
    assert not index.exists()


def test_receipt_removal_that_cannot_move_now_is_deferred_and_retried(
    tmp_path, monkeypatch
):
    lease, _ = _lease(tmp_path)
    _finish(lease, success=False)
    move = scratch.namespace_move_exclusive

    def contended(source, destination):
        if source == lease.generation:
            raise PermissionError("fixture: a contender holds the generation lock")
        return move(source, destination)

    monkeypatch.setattr(scratch, "namespace_move_exclusive", contended)
    first = scratch.reclaim_terminal_scratch(
        lease.generation.parent, retention=scratch.ScratchRetention(0, 0)
    )
    assert first["errors"] == []
    assert first["deferred"] and "contender" in first["deferred"][0]
    assert _read(lease.generation / "owner.json")["state"] == "reclaimed"
    assert scratch._index_path(lease.generation).exists()
    monkeypatch.setattr(scratch, "namespace_move_exclusive", move)
    second = scratch.reclaim_terminal_scratch(lease.generation.parent)
    assert second["errors"] == [] and second["deferred"] == []
    assert not lease.generation.exists()
    assert not scratch._index_path(lease.generation).exists()


def test_process_scratch_skips_fsync_but_failure_evidence_is_durable(
    tmp_path, monkeypatch
):
    """Scratch records describe processes, so none crosses a barrier (HF-382).

    An OS crash that drops one also killed every process it names; the
    crash-residue tests below prove that each such state stays safe.
    """
    fsyncs = []
    real_fsync = os.fsync
    # Only file publication issues fsync on these paths; count it there
    # without faking os.fsync for the whole process.
    install_module_view(
        monkeypatch,
        "os",
        os,
        file_publication,
        fsync=lambda fd: fsyncs.append(fd) or real_fsync(fd),
    )
    lease, _ = _lease(tmp_path)
    assert fsyncs == []
    result = _finish(lease)
    assert result["state"] == "reclaimed" and not lease.generation.exists()
    assert fsyncs == []
    failed, _ = _lease(tmp_path, 2)
    assert _finish(failed, success=False)["state"] == "retained"
    adopted, env = _lease(tmp_path, 3)
    adopted.release()
    assert scratch.resolve_guard_scratch(
        adopted.generation,
        guard_marker=Path(env["MOLT_MEMORY_GUARD_MARKER"]),
        closure={"authority": "fixture", "closed": True},
    )["resolved"]
    assert fsyncs  # Failed evidence is durable before retirement is granted.
    archive_root = Path(failed.owner["evidence_root"])
    swept = scratch.reclaim_terminal_scratch(
        lease.generation.parent, retention=scratch.ScratchRetention(0, 0)
    )
    assert swept["errors"] == [] and len(swept["reclaimed"]) == 2
    assert fsyncs  # Evidence/index durability precedes raw failure removal.
    assert len(list(archive_root.glob("*.index.json"))) == 2


def test_a_terminal_receipt_its_owner_never_committed_blocks_adoption_once(
    tmp_path, monkeypatch
):
    """The finisher wrote its terminal receipt, then failed before its index.

    A dead guard's reconciliation then met that receipt on every pass: its
    adoption tried to publish a second terminal receipt and failed, so the
    marker never resolved. It now verifies the receipt as the finisher's own,
    blocks the generation once and keeps the payload.
    """
    lease, env = _lease(tmp_path)
    (lease.target / "output").write_bytes(b"kept")

    def fail_index(generation, digest):
        raise OSError("fixture index publication failure")

    monkeypatch.setattr(scratch, "_publish_index", fail_index)
    with pytest.raises(OSError, match="index publication failure"):
        _finish(lease)
    monkeypatch.undo()
    assert (lease.generation / "terminal.json").is_file()
    assert _read(lease.generation / "owner.json")["state"] == "indeterminate"
    outcomes = [
        scratch.resolve_guard_scratch(
            lease.generation, guard_marker=_marker_of(env), closure=_CLOSURE
        )
        for _ in range(2)
    ]
    assert [(item["state"], item["resolved"]) for item in outcomes] == [
        ("blocked", True),
        ("blocked", True),
    ]
    assert (lease.target / "output").read_bytes() == b"kept"
    assert scratch.reclaim_terminal_scratch(lease.generation.parent)["errors"] == []


# --- Crash residue (HF-382) -------------------------------------------------
#
# The records cross no durability barrier, so an OS crash can persist any
# subset of the protocol's writes in any order, and a file whose rename
# persisted before its data reads back empty or torn. The model rebuilds
# every combination of the records a finisher writes, from the bytes the real
# protocol wrote, and runs both post-crash readers on it: the pending sweep
# and the reconciliation of the dead guard's marker. After an OS crash every
# process of the run is dead, so the oracle is custody policy, not liveness:
# nothing but the run's own payload is deleted, an interrupted deletion is
# never retried, a payload without a valid terminal receipt is never deleted,
# every surviving payload or generation is reported, and nothing is reported
# twice (a repeated report fails every later guarded command with 125).

_MODEL_TARGET = "pt-model000"
_MODEL_NONCE = "c" * 32
# Owner generations a finisher writes: leased, indeterminate (unproven
# closure), retiring, retained, reclaiming, reclaimed. An undecodable record
# is empty or "torn" (a prefix); readers take one path for both, so each
# record carries one of them. An index stage is never read, only replaced.
_MODEL_OWNERS = ("absent", "empty", "L", "I", "T", "R", "C", "D")
_MODEL_TERMINALS = ("absent", "torn", "oversized", "valid")
_MODEL_INDEXES = ("absent", "stage", "final", "final-empty")
_MODEL_PAYLOADS = ("target", "target-partial", "payload", "payload-partial", "gone")


def _model_templates(root: Path, monkeypatch) -> tuple[dict[str, object], Path]:
    """Capture every record the real protocol writes for one generation."""
    allocate = scratch.new_temporary_directory

    def fixed_target(parent, *, prefix="tmp"):
        path = allocate(parent, prefix=prefix)
        if prefix != "pt-":
            return path
        return path.rename(parent / _MODEL_TARGET)

    writes: list[tuple[str, dict[str, object]]] = []
    write = scratch.write_exact

    def capture(path, value, **kwargs):
        writes.append((Path(path).name, json.loads(json.dumps(value))))
        write(path, value, **kwargs)

    with monkeypatch.context() as patch:
        patch.setattr(scratch, "new_temporary_directory", fixed_target)
        patch.setattr(scratch, "write_exact", capture)
        lease, env = _lease(root)
        _finish(lease)
        shutil.rmtree(root)
        lease, env = _lease(root)
        _finish(lease, closed=False)
        lease.release()
    shutil.rmtree(root)
    owners = [value for name, value in writes if name == "owner.json"]
    states = [owner["state"] for owner in owners]
    assert states == [
        "leased",
        "retiring",
        "retained",
        "reclaiming",
        "reclaimed",
        "leased",
        "indeterminate",
    ], states
    (terminal,) = [value for name, value in writes if name == "terminal.json"]
    (index,) = [value for name, value in writes if name == "pending.json"]
    templates = dict(zip("LTRCD", owners[:5], strict=True))
    templates["I"] = owners[6]
    return {"owners": templates, "terminal": terminal, "index": index}, _marker_of(env)


def _model_payload(directory: Path, *, partial: bool) -> None:
    directory.mkdir()
    scratch._write_target_receipt(directory, _MODEL_NONCE)
    (directory / "a.bin").write_bytes(b"a" * 64)
    (directory / "b.bin").write_bytes(b"b" * 64)
    if partial:
        # An interrupted deletion: some entries, the receipt among them, gone.
        (directory / ".molt-scratch-target.json").unlink()
        (directory / "a.bin").unlink()


def _model_bytes(value: object, form: str) -> bytes:
    if form == "oversized":
        # Decodable JSON past the receipt bound, as an unbounded closure once
        # wrote (HF-479): its generation blocks alone, never every sweep.
        assert isinstance(value, dict)
        return scratch.encode_exact({**value, "pad": "x" * scratch._MAX_RECEIPT_BYTES})
    data = scratch.encode_exact(value)
    return {"empty": b"", "torn": data[: len(data) // 2]}.get(form, data)


def _model_may_delete(owner: str, terminal: str, payload: str) -> bool:
    """Deletion custody policy for one crash state; liveness is moot."""
    if owner in {"T", "R"}:
        # A terminal state deletes only the nested payload of a valid receipt.
        return terminal == "valid" and payload in {"payload", "payload-partial"}
    if owner in {"L", "I"}:
        # Adoption deletes only the intact lease target it can identify.
        return payload == "target"
    # Reclaiming or reclaimed receipts show a deletion that already started;
    # an unreadable owner grants nothing.
    return False


def _run_crash_state(
    root: Path,
    templates: dict[str, object],
    marker: Path,
    *,
    owner: str,
    terminal: str,
    index: str,
    payload: str,
    lock: bool,
    sweep_first: bool,
) -> list[str]:
    if root.exists():
        shutil.rmtree(root)
    scratch_root = root / "tmp" / "gs"
    generation = scratch_root / f"{1:032x}"
    generation.mkdir(parents=True)
    if lock:
        file_locks._release_file_lock(
            file_locks._try_acquire_file_lock(generation / "lock")
        )
    foreign = root / "tmp" / "pt-foreign0"
    foreign.mkdir()
    (foreign / "keep").write_bytes(b"keep")
    locations = {
        "target": root / "tmp" / _MODEL_TARGET,
        "payload": generation / "payload",
    }
    identity = {"device": 0, "inode": 1}
    if payload != "gone":
        directory = locations[payload.removesuffix("-partial")]
        _model_payload(directory, partial=payload.endswith("-partial"))
        identity = scratch._identity(directory)
    receipt = {**templates["terminal"], "target_identity": identity}
    digest = canonical_json_sha256(receipt)
    owners = {
        name: {
            **value,
            "target_identity": identity,
            "target_receipt": _MODEL_NONCE,
            **({"terminal_digest": digest} if "terminal_digest" in value else {}),
        }
        for name, value in templates["owners"].items()
    }
    if owner != "absent":
        value = owners.get(owner, owners["L"])
        (generation / "owner.json").write_bytes(_model_bytes(value, owner))
    if terminal != "absent":
        (generation / "terminal.json").write_bytes(_model_bytes(receipt, terminal))
    if index != "absent":
        path = (
            generation / "pending.json"
            if index.startswith("stage")
            else scratch._index_path(generation)
        )
        path.parent.mkdir(exist_ok=True)
        path.write_bytes(
            _model_bytes(
                {**templates["index"], "terminal_digest": digest},
                index.removeprefix("stage").removeprefix("final").lstrip("-")
                or "valid",
            )
        )

    def survives() -> bool:
        for path in (
            *locations.values(),
            scratch_root / "removing" / generation.name / "payload",
        ):
            try:
                if scratch._identity(path) == identity:
                    return True
            except (OSError, ValueError):
                continue
        return False

    problems: list[str] = []
    sweep_reports: list[str] = []
    resolved: list[dict[str, object]] = []

    def sweep() -> None:
        result = scratch.reclaim_terminal_scratch(
            scratch_root, retention=scratch.ScratchRetention(0, 0)
        )
        sweep_reports.extend(str(error) for error in result["errors"])

    def resolve() -> None:
        # The marker retires once its scratch resolves; then no one asks.
        if resolved and resolved[-1]["resolved"]:
            return
        result = scratch.resolve_guard_scratch(
            generation, guard_marker=marker, closure=_CLOSURE
        )
        if result["state"] == "error":
            problems.append(f"reconciliation failed: {result.get('error')}")
        resolved.append(result)

    # Three passes of both readers: every guard exit sweeps, and the next
    # reconciliation retries an unresolved marker.
    for _ in range(3):
        try:
            for step in (sweep, resolve) if sweep_first else (resolve, sweep):
                step()
        except Exception as error:  # Any escape is a defect of the reader.
            problems.append(f"reader raised {type(error).__name__}: {error}")
            return problems
    if len(sweep_reports) > 1:
        problems.append(f"the sweep reported more than once: {sweep_reports}")
    if not resolved[-1]["resolved"]:
        problems.append(f"the marker never resolves: {resolved[-1]}")
    if payload != "gone" and not survives():
        if not _model_may_delete(owner, terminal, payload):
            problems.append("deleted a payload its receipts do not authorize")
    reported = bool(sweep_reports) or any(
        item["state"] == "blocked" or "error" in item for item in resolved
    )
    if payload != "gone" and survives() and not reported:
        problems.append("payload survives without a report")
    if os.path.lexists(generation) and not reported:
        problems.append("generation survives without a report")
    if not (foreign / "keep").is_file():
        problems.append("deleted a directory outside the generation")
    try:
        written = _read(generation / "terminal.json")
    except (OSError, ValueError):
        written = None
    if isinstance(written, dict) and written.get("source_target_absent") and survives():
        problems.append("a terminal receipt calls a surviving payload absent")
    return problems


@pytest.mark.parametrize("sweep_first", [True, False], ids=["sweep", "resolve"])
@pytest.mark.parametrize("owner", _MODEL_OWNERS)
def test_every_crash_state_keeps_custody_and_reports_once(
    tmp_path, monkeypatch, owner, sweep_first
):
    root = tmp_path / "m"
    templates, marker = _model_templates(root, monkeypatch)
    failures = []
    for terminal in _MODEL_TERMINALS:
        for index in _MODEL_INDEXES:
            for payload in _MODEL_PAYLOADS:
                for problem in _run_crash_state(
                    root,
                    templates,
                    marker,
                    owner=owner,
                    terminal=terminal,
                    index=index,
                    payload=payload,
                    lock=True,
                    sweep_first=sweep_first,
                ):
                    failures.append(
                        f"owner={owner} terminal={terminal} index={index} "
                        f"payload={payload}: {problem}"
                    )
    assert failures == []


@pytest.mark.parametrize("sweep_first", [True, False], ids=["sweep", "resolve"])
def test_a_crash_that_drops_the_generation_lock_is_reported_once(
    tmp_path, monkeypatch, sweep_first
):
    root = tmp_path / "m"
    templates, marker = _model_templates(root, monkeypatch)
    failures = []
    for owner in ("absent", "L", "R", "D"):
        for index in ("absent", "final"):
            for payload in ("target", "payload", "gone"):
                for problem in _run_crash_state(
                    root,
                    templates,
                    marker,
                    owner=owner,
                    terminal="valid",
                    index=index,
                    payload=payload,
                    lock=False,
                    sweep_first=sweep_first,
                ):
                    failures.append(
                        f"owner={owner} index={index} payload={payload}: {problem}"
                    )
    assert failures == []


def test_same_spelling_reuses_validation_but_never_admits_a_link(tmp_path, monkeypatch):
    target = tmp_path / "owned"
    target.mkdir()
    calls = []
    original = scratch.resolve_owned_path

    def resolve(path):
        calls.append(path)
        return original(path)

    monkeypatch.setattr(scratch, "resolve_owned_path", resolve)
    assert scratch._same_owned_path(str(target), str(target))
    assert calls == [target]
    link = tmp_path / "link"
    link.symlink_to(target, target_is_directory=True)
    with pytest.raises((ValueError, OSError)):
        scratch._same_owned_path(str(link), str(link))
    assert calls[-1] == link


def test_owned_temporary_directory_explicit_retention_and_later_cleanup(tmp_path):
    import gc

    owner = scratch.OwnedTemporaryDirectory(dir=tmp_path)
    retained = owner.retain()
    (retained / "evidence").write_bytes(b"live-child-output")
    del owner
    gc.collect()
    assert (retained / "evidence").read_bytes() == b"live-child-output"
    # Retention detaches automatic reclamation, not explicit ownership cleanup.
    another = scratch.OwnedTemporaryDirectory(dir=tmp_path)
    path = another.retain()
    another.cleanup()
    assert not path.exists()


@pytest.mark.parametrize(
    "failure_phase",
    [
        None,
        "terminal_publication",
        "payload_reclamation",
        "receipt_removal",
        "retention_sweep",
        "lease_release",
    ],
)
def test_scratch_finalization_times_real_phase_boundaries_on_failure(
    tmp_path, monkeypatch, failure_phase
):
    import time

    now = [100.0]
    events = []
    failure = RuntimeError("injected finalization boundary")
    original_lease = scratch.GuardScratchLease

    class TimedLease(scratch.GuardScratchLease):
        def release(self):
            events.append("lease_release")
            try:
                super().release()
                if failure_phase == "lease_release":
                    raise failure
            finally:
                now[0] += 11.0

    # Substitute only the module constructor seam, never the shared class.
    monkeypatch.setattr(
        scratch, "GuardScratchLease", current_thread_only(TimedLease, original_lease)
    )
    lease, _ = _lease(tmp_path)
    install_thread_module_view(
        monkeypatch, "time", time, scratch, perf_counter=lambda: now[0]
    )

    def timed(operation, label, cost):
        def run(*args, **kwargs):
            events.append(label)
            try:
                if failure_phase == label:
                    raise failure
                return operation(*args, **kwargs)
            finally:
                now[0] += cost

        return current_thread_only(run, operation)

    for name, label, cost in (
        ("_retire_locked", "terminal_publication", 2.0),
        ("_reclaim_locked", "payload_reclamation", 3.0),
        ("_remove_reclaimed_generation", "receipt_removal", 5.0),
        ("reclaim_terminal_scratch", "retention_sweep", 7.0),
    ):
        monkeypatch.setattr(scratch, name, timed(getattr(scratch, name), label, cost))
    original_owner = scratch._owner
    admission_reads = []

    def admitted_owner(generation):
        first_read = not admission_reads
        admission_reads.append(generation)
        try:
            return original_owner(generation)
        finally:
            # Only the initial real owner admission costs time. Later final-state
            # and error-receipt reads must not be charged to that boundary again.
            if first_read:
                now[0] += 13.0

    monkeypatch.setattr(
        scratch, "_owner", current_thread_only(admitted_owner, original_owner)
    )
    timings = scratch.ScratchFinalizationTimings()
    expected = {
        "terminal_publication": 15.0,
        "payload_reclamation": None,
        "receipt_removal": None,
        "retention_sweep": None,
        "lease_release": 11.0,
    }
    expected_events = ["terminal_publication"]
    if failure_phase != "terminal_publication":
        expected["payload_reclamation"] = 3.0
        expected_events.append("payload_reclamation")
    expected_events.append("lease_release")
    if failure_phase not in {
        "terminal_publication",
        "payload_reclamation",
        "lease_release",
    }:
        expected["receipt_removal"] = 5.0
        expected_events.append("receipt_removal")
        if failure_phase != "receipt_removal":
            expected["retention_sweep"] = 7.0
            expected_events.append("retention_sweep")
    try:
        if failure_phase is None:
            outcome = _finish(lease, timings=timings)
            assert outcome["state"] == "reclaimed"
            assert outcome["receipt"] is None
            assert outcome["retention"]["errors"] == []
        else:
            with pytest.raises(RuntimeError) as caught:
                _finish(lease, timings=timings)
            assert caught.value is failure
        assert admission_reads[0] == lease.generation
        assert timings.json_payload() == expected
        assert events == expected_events
        assert lease.lock is None
        # Failed admission retains the original payload; failure after retirement
        # retains its moved payload. Later failures never resurrect deleted data.
        assert lease.target.exists() is (failure_phase == "terminal_publication")
        assert (lease.generation / "payload").exists() is (
            failure_phase == "payload_reclamation"
        )
        assert "finalize_phase_elapsed_s" not in lease.owner
        for name in ("owner.json", "terminal.json"):
            receipt = lease.generation / name
            if receipt.exists():
                assert "finalize_phase_elapsed_s" not in _read(receipt)
                assert "timings" not in _read(receipt)
    finally:
        original_lease.release(lease)


@pytest.mark.parametrize("closed", [False, True])
def test_scratch_finalization_distinguishes_unentered_and_zero_cost_phases(
    tmp_path, monkeypatch, closed
):
    import time

    lease, _ = _lease(tmp_path)
    install_thread_module_view(
        monkeypatch, "time", time, scratch, perf_counter=lambda: 42.0
    )
    timings = scratch.ScratchFinalizationTimings()
    outcome = _finish(lease, closed=closed, success=False, timings=timings)
    assert outcome["state"] == ("retained" if closed else "indeterminate")
    assert timings.json_payload() == {
        "terminal_publication": 0.0,
        "payload_reclamation": None,
        "receipt_removal": None,
        "retention_sweep": 0.0 if closed else None,
        "lease_release": 0.0,
    }
    assert lease.target.exists() is (not closed)
    assert (lease.generation / "payload").exists() is closed


def test_scratch_release_timing_adds_sequential_attempts_without_hiding_failure(
    monkeypatch,
):
    import time

    now = [10.0]
    install_thread_module_view(
        monkeypatch, "time", time, scratch, perf_counter=lambda: now[0]
    )
    timings = scratch.ScratchFinalizationTimings()
    phase = scratch.ScratchFinalizationPhase.LEASE_RELEASE
    with timings.measure(phase):
        now[0] += 2.0
    failure = OSError("release failed")
    with pytest.raises(OSError) as caught:
        with timings.measure(phase):
            now[0] += 3.0
            raise failure
    assert caught.value is failure
    assert timings.json_payload() == {
        "terminal_publication": None,
        "payload_reclamation": None,
        "receipt_removal": None,
        "retention_sweep": None,
        "lease_release": 5.0,
    }


def test_failed_scratch_reclamation_publishes_lossless_external_evidence(tmp_path):
    import tarfile

    lease, _ = _lease(tmp_path)
    payload = b"unreproducible failure signal" * 100
    (lease.target / "failure.bin").write_bytes(payload)
    archive_root = Path(lease.owner["evidence_root"])
    result = _finish(lease, success=False, retention=scratch.ScratchRetention(0, 0))
    assert result["state"] == "reclaimed"
    assert not lease.generation.exists()
    manifests = list(archive_root.glob("*.index.json"))
    assert len(manifests) == 1
    record = read_exact(manifests[0], max_bytes=1024 * 1024, label="archive manifest")
    assert record["class"] == "scratch-evidence"
    assert record["source"] == str(lease.generation)
    with tarfile.open(record["archive"], "r:gz") as archive:
        assert archive.extractfile("payload/failure.bin").read() == payload
        assert archive.extractfile("terminal.json") is not None


def test_legacy_failed_scratch_without_archive_placement_is_never_deleted(tmp_path):
    lease, _ = _lease(tmp_path)
    (lease.target / "failure").write_bytes(b"legacy evidence")
    lease.owner.pop("evidence_root")
    write_exact(lease.generation / "owner.json", lease.owner)
    result = _finish(lease, success=False, retention=scratch.ScratchRetention(0, 0))
    assert result["state"] == "blocked"
    assert (lease.generation / "payload" / "failure").read_bytes() == b"legacy evidence"
    scratch.reclaim_terminal_scratch(lease.generation.parent)
    assert (lease.generation / "payload" / "failure").read_bytes() == b"legacy evidence"


def test_failed_archive_error_preserves_raw_evidence(tmp_path, monkeypatch):
    from molt import checkout_state

    lease, _ = _lease(tmp_path)
    (lease.target / "failure").write_bytes(b"retain on storage failure")

    def unavailable(*args, **kwargs):
        raise OSError("archive volume unavailable")

    monkeypatch.setattr(checkout_state, "compact_evidence", unavailable)
    result = _finish(lease, success=False, retention=scratch.ScratchRetention(0, 0))
    assert result["state"] == "blocked"
    assert (
        lease.generation / "payload" / "failure"
    ).read_bytes() == b"retain on storage failure"
    assert "archive volume unavailable" in result["error"]
    assert not scratch._index_path(lease.generation).exists()


def test_deferred_failed_receipts_reuse_verified_archive_and_preserve_new_signal(
    tmp_path, monkeypatch
):
    lease, _ = _lease(tmp_path)
    (lease.target / "failure").write_bytes(b"unique failure")
    monkeypatch.setattr(
        scratch, "_remove_reclaimed_generation", lambda generation: "fixture deferral"
    )
    result = _finish(lease, success=False, retention=scratch.ScratchRetention(0, 0))
    assert result["state"] == "reclaimed"
    archive_root = Path(lease.owner["evidence_root"])
    assert len(list(archive_root.glob("*.tar.gz"))) == 1
    with scratch._locked(lease.generation):
        scratch._reclaim_locked(lease.generation, scratch._owner(lease.generation))
    assert len(list(archive_root.glob("*.tar.gz"))) == 1
    signal = lease.generation / "late-diagnostic"
    signal.write_bytes(b"must not disappear")
    with scratch._locked(lease.generation):
        with pytest.raises(ValueError, match="diagnostics changed"):
            scratch._reclaim_locked(lease.generation, scratch._owner(lease.generation))
    assert signal.read_bytes() == b"must not disappear"


def test_legacy_reclaimed_failure_keeps_remaining_diagnostic_records(tmp_path):
    lease, env = _lease(tmp_path)
    lease.owner.pop("evidence_root")
    write_exact(lease.generation / "owner.json", lease.owner)
    _finish(lease, success=False)
    shutil.rmtree(
        lease.generation / "payload"
    )  # A legacy collector already removed it.
    owner = scratch._owner(lease.generation)
    write_exact(lease.generation / "owner.json", {**owner, "state": "reclaimed"})
    original_terminal = (lease.generation / "terminal.json").read_bytes()
    result = scratch.resolve_guard_scratch(
        lease.generation, guard_marker=_marker_of(env), closure=_CLOSURE
    )
    assert result["resolved"] and result["state"] == "retained-evidence"
    swept = scratch.reclaim_terminal_scratch(lease.generation.parent)
    assert swept["reclaimed"] == []
    assert "legacy failed scratch" in result["error"]
    assert (lease.generation / "terminal.json").read_bytes() == original_terminal


def test_failed_retirement_grant_already_has_verified_archive_under_live_lease(
    tmp_path, monkeypatch
):
    import tarfile

    lease, _ = _lease(tmp_path)
    payload = b"must survive an older collector"
    (lease.target / "failure").write_bytes(payload)
    states = []
    original = scratch.write_exact

    def observe(path, value, **kwargs):
        if path == lease.generation / "owner.json":
            states.append(value["state"])
            if value["state"] == "retained":
                assert file_locks._file_lock_is_owned(lease.lock)
                index = read_exact(
                    Path(value["evidence_index"]),
                    max_bytes=1024 * 1024,
                    label="failure archive",
                )
                with tarfile.open(index["archive"]) as archive:
                    assert archive.extractfile("payload/failure").read() == payload
        return original(path, value, **kwargs)

    monkeypatch.setattr(scratch, "write_exact", observe)
    result = _finish(lease, success=False)
    assert result["state"] == "retained"
    assert states[0] == "blocked" and states[-1] == "retained"
    assert "retiring" not in states
    index = Path(scratch._owner(lease.generation)["evidence_index"])
    before = set(index.parent.glob("*.tar.gz"))
    scratch.reclaim_terminal_scratch(
        lease.generation.parent, retention=scratch.ScratchRetention(0, 0)
    )
    assert not lease.generation.exists()
    assert set(index.parent.glob("*.tar.gz")) == before


def test_changed_retained_directory_link_is_archived_before_reclamation(tmp_path):
    import tarfile

    first, second = tmp_path / "first", tmp_path / "second"
    for directory in (first, second):
        directory.mkdir()
        (directory / "keep").write_bytes(b"external directory")
    lease, _ = _lease(tmp_path)
    (lease.target / "link").symlink_to(first, target_is_directory=True)
    _finish(lease, success=False)
    link = lease.generation / "payload" / "link"
    link.unlink()
    link.symlink_to(second, target_is_directory=True)
    archive_root = Path(lease.owner["evidence_root"])
    result = scratch.reclaim_terminal_scratch(
        lease.generation.parent, retention=scratch.ScratchRetention(0, 0)
    )
    assert result["errors"] == [] and result["reclaimed"] == [str(lease.generation)]
    records = [
        read_exact(p, max_bytes=1024 * 1024, label="link evidence")
        for p in archive_root.glob("*.index.json")
    ]
    assert len(records) == 2
    latest = next(record for record in records if "previous_index" in record)
    with tarfile.open(latest["archive"]) as archive:
        assert archive.getmember("payload/link").linkname == str(second)
    for directory in (first, second):
        assert (directory / "keep").read_bytes() == b"external directory"
