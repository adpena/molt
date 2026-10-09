from __future__ import annotations

from contextlib import contextmanager
import json
import os
from pathlib import Path

import pytest

from molt import temporary_artifacts as scratch
from molt import file_locks
from molt.exact_json import canonical_json_sha256, read_exact, write_exact
from tests.process_guard_common import install_module_view


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
        assert scratch.guard_scratch(tmp_path, env) == lease.target
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
        assert scratch.guard_scratch(tmp_path, env) == lease.target
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
    assert _read(terminal_path) == terminal
    assert not (lease.generation / "payload").exists()


def test_parent_allocation_binds_consumption_and_terminal_cleanup(tmp_path):
    lease, env = _lease(tmp_path)
    legacy = lease.target.parent / "pt-abcdefgh"
    legacy.mkdir()
    (legacy / "keep").write_text("legacy", encoding="utf-8")
    (lease.target / "output").write_bytes(b"scratch")
    assert scratch.guard_scratch(tmp_path, env) == lease.target
    result = _finish(lease)
    assert result["state"] == "reclaimed"
    assert lease.lock is None
    assert not lease.target.exists()
    assert not (lease.generation / "payload").exists()
    assert (legacy / "keep").read_text(encoding="utf-8") == "legacy"
    assert _read(lease.generation / "terminal.json")["source_target"] == str(
        lease.target
    )
    with pytest.raises(ValueError, match="active parent's"):
        scratch.guard_scratch(tmp_path, env)


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
            scratch.guard_scratch(tmp_path, env)
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
            scratch.guard_scratch(tmp_path, env)
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
            scratch.guard_scratch(tmp_path, env)
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
            scratch.guard_scratch(tmp_path, env)
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


def test_pending_publication_exposes_only_committed_locked_generations(
    tmp_path, monkeypatch
):
    from molt import file_publication

    prior, _ = _lease(tmp_path)
    _finish(prior, success=False)
    active, _ = _lease(tmp_path, 2)
    root = active.generation.parent
    publish = file_publication.durable_publish_exclusive
    checkpoints = []
    discovered = []

    def publish_private(staged, destination):
        if destination == active.generation / "pending.json":
            assert staged.parent == active.generation
            result = scratch.reclaim_terminal_scratch(root)
            assert result["errors"] == []
            assert result["retained_count"] == 1
            assert result["protected_count"] == 0
            checkpoints.append("private-stage")
        publish(staged, destination)

    def publish_index(staged, destination):
        assert staged == active.generation / "pending.json"
        assert destination == scratch._index_path(active.generation)
        publish(staged, destination)
        discovered.extend(destination.parent.iterdir())
        result = scratch.reclaim_terminal_scratch(root)
        assert result["errors"] == []
        assert result["retained_count"] == 1
        assert result["protected_count"] == 1
        checkpoints.append("published-with-owner-lock")

    monkeypatch.setattr(file_publication, "durable_publish_exclusive", publish_private)
    monkeypatch.setattr(scratch, "durable_publish_exclusive", publish_index)
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

    monkeypatch.setattr(scratch, "_publish_index", publish)
    thread = threading.Thread(target=transfer)
    thread.start()
    try:
        result = _finish(lease)
        assert result["state"] == "reclaimed"
        assert transferred.wait(5)
        assert contender[0] is not None
        assert not file_locks._file_lock_is_owned(handle)
        assert file_locks._file_lock_is_owned(contender[0])
        assert _read(lease.generation / "owner.json")["state"] == "reclaimed"
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
