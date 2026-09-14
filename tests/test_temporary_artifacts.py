from __future__ import annotations

from pathlib import Path

import pytest

from molt import temporary_artifacts as scratch
from molt.exact_json import canonical_json_sha256, read_exact, write_exact


def _lease(root: Path, number: int = 1):
    state = root / "tmp" / "memory_guard"
    env = {
        "MOLT_MEMORY_GUARD_STATE_ROOT": str(state),
        "MOLT_MEMORY_GUARD_TOKEN": f"{number:032x}",
        "MOLT_MEMORY_GUARD_MARKER": str(state / "active" / f"guard-{number}.json"),
    }
    lease = scratch.acquire_guard_scratch(root, env)
    env[scratch.SCRATCH_ENV] = str(lease.target)
    return lease, env


def _read(path: Path):
    return read_exact(path, max_bytes=65536, label="test scratch receipt")


def _finish(lease, *, success=True, closed=True, **kwargs):
    return scratch.finish_guard_scratch(
        lease,
        success=success,
        closed=closed,
        evidence={"authority": "fixture", "child_returncode": 0 if success else 1},
        **kwargs,
    )


def test_parent_allocation_binds_consumption_and_terminal_cleanup(tmp_path):
    lease, env = _lease(tmp_path)
    legacy = lease.target.parent / "pt-abcdefgh"
    legacy.mkdir()
    (legacy / "keep").write_text("legacy")
    (lease.target / "output").write_bytes(b"scratch")
    assert scratch.guard_scratch(tmp_path, env) == lease.target
    result = _finish(lease)
    assert result["state"] == "reclaimed"
    assert lease.lock is None
    assert not lease.target.exists()
    assert not (lease.generation / "payload").exists()
    assert (legacy / "keep").read_text() == "legacy"
    assert _read(lease.generation / "terminal.json")["source_target"] == str(
        lease.target
    )
    with pytest.raises(ValueError, match="active parent's"):
        scratch.guard_scratch(tmp_path, env)


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
    assert (first.generation / "terminal.json").is_file()
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

    monkeypatch.setattr(scratch, "_target_bytes", fail_measurement)
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


def test_missing_retained_payload_is_reported_not_counted_as_retained(tmp_path):
    lease, _ = _lease(tmp_path)
    _finish(lease, success=False)
    (lease.generation / "payload").rmdir()
    result = scratch.reclaim_terminal_scratch(lease.generation.parent)
    assert result["errors"] and result["retained_count"] == 0


@pytest.mark.parametrize("present", [False, True])
def test_interrupted_retirement_requires_nested_payload_identity(tmp_path, present):
    lease, _ = _lease(tmp_path)
    _finish(lease, success=False)
    payload = lease.generation / "payload"
    if not present:
        payload.rmdir()
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
        target.rmdir()
    owner = _read(lease.generation / "owner.json")
    write_exact(lease.generation / "owner.json", dict(owner, state="reclaiming"))
    monkeypatch.setattr(
        scratch, "delete_path", lambda *_: pytest.fail("must not retry")
    )
    scratch.reclaim_terminal_scratch(
        lease.generation.parent, retention=scratch.ScratchRetention(0, 0)
    )
    assert _read(lease.generation / "owner.json")["state"] == (
        "blocked" if present else "reclaimed"
    )


def test_delete_failure_is_durable_and_not_silenced(tmp_path, monkeypatch):
    lease, _ = _lease(tmp_path)
    monkeypatch.setattr(
        scratch, "delete_path", lambda *_: (False, "fixture sharing violation")
    )
    result = _finish(lease)
    assert result["state"] == "blocked"
    assert "sharing violation" in _read(lease.generation / "owner.json")["error"]
    assert (lease.generation / "payload").exists()


def test_oversized_failed_payload_is_reclaimed_but_receipt_survives(tmp_path):
    lease, _ = _lease(tmp_path)
    (lease.target / "output").write_bytes(b"1234")
    result = _finish(lease, success=False, retention=scratch.ScratchRetention(3, 3))
    assert result["state"] == "reclaimed"
    assert _read(lease.generation / "terminal.json")["retained_bytes"] == 4


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
            scratch.new_guarded_directory(tmp_path, env, prefix=prefix)
    finally:
        _finish(lease)


def test_helper_subdirectories_are_owned_by_terminal_guard_not_context_age(tmp_path):
    lease, env = _lease(tmp_path)
    first = scratch.new_guarded_directory(tmp_path, env, prefix="compile-")
    second = scratch.new_guarded_directory(tmp_path, env, prefix="compile-")
    assert first.parent == second.parent == lease.target
    assert first != second
    _finish(lease)
    assert not first.exists() and not second.exists()


@pytest.mark.parametrize("count,bytes_", [(-1, 1), (1, -1), (True, 0), (1, 1.5)])
def test_retention_limits_are_exact_nonnegative_integers(count, bytes_):
    with pytest.raises(ValueError):
        scratch.ScratchRetention(count, bytes_)
