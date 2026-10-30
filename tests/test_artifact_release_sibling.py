from __future__ import annotations
from types import SimpleNamespace
import pytest
from molt import artifact_publication as publication


def setup_model(monkeypatch, tmp_path):
    parents = {tmp_path / "one", tmp_path / "two"}
    handles = []

    def acquire(*args, **kwargs):
        handle = SimpleNamespace(
            file=SimpleNamespace(fileno=lambda: 1), number=len(handles)
        )
        handles.append(handle)
        return handle

    monkeypatch.setattr(
        publication, "_verify_publication_lock", lambda *args, **kwargs: None
    )
    monkeypatch.setattr(publication, "_acquire_file_lock", acquire)
    monkeypatch.setattr(
        publication, "_journal_parent_closure", lambda current: (current, {})
    )
    return parents, handles


def test_single_cleanup_failure_attempts_remaining_handles(monkeypatch, tmp_path):
    parents, handles = setup_model(monkeypatch, tmp_path)
    attempted = []
    failure = OSError("unlock fault")

    def release(handle):
        attempted.append(handle.number)
        if handle.number == 1:
            raise failure

    monkeypatch.setattr(publication, "_release_file_lock", release)
    with pytest.raises(OSError) as caught:
        with publication._publication_locks(parents):
            pass
    assert caught.value is failure and attempted == [1, 0]


def test_primary_callback_failure_is_preserved_with_cleanup_notes(
    monkeypatch, tmp_path
):
    parents, handles = setup_model(monkeypatch, tmp_path)
    attempted = []
    primary = ValueError("callback failure")

    def release(handle):
        attempted.append(handle.number)
        raise OSError("cleanup fault")

    monkeypatch.setattr(publication, "_release_file_lock", release)
    with pytest.raises(ValueError) as caught:
        with publication._publication_locks(parents):
            raise primary
    assert caught.value is primary and attempted == [1, 0]
    assert len(primary.__notes__) == 2


def test_multiple_cleanup_failures_keep_every_original_exception(monkeypatch, tmp_path):
    parents, handles = setup_model(monkeypatch, tmp_path)
    attempted = []
    failures = [OSError("close"), KeyboardInterrupt("interrupted")]

    def release(handle):
        attempted.append(handle.number)
        raise failures[handle.number]

    monkeypatch.setattr(publication, "_release_file_lock", release)
    with pytest.raises(BaseExceptionGroup) as caught:
        with publication._publication_locks(parents):
            pass
    assert attempted == [1, 0] and tuple(caught.value.exceptions) == (
        failures[1],
        failures[0],
    )


def test_actual_remaining_publication_locks_close_after_postclose_error(
    monkeypatch, tmp_path
):
    from molt.file_locks import _try_acquire_file_lock, _release_file_lock

    parents = {tmp_path / "one", tmp_path / "two"}
    for parent in parents:
        parent.mkdir()
    monkeypatch.setattr(
        publication, "_journal_parent_closure", lambda current: (current, {})
    )
    original = publication._release_file_lock
    attempted = []

    def release(handle):
        attempted.append(handle)
        original(handle)
        if len(attempted) == 1:
            raise OSError("post-close callback fault")

    monkeypatch.setattr(publication, "_release_file_lock", release)
    with pytest.raises(OSError, match="post-close"):
        with publication._publication_locks(parents):
            pass
    assert len(attempted) == 2
    for parent in parents:
        handle = _try_acquire_file_lock(publication._publication_lock_path(parent))
        assert handle is not None
        _release_file_lock(handle)


def test_outer_handled_exception_does_not_hide_cleanup_failure(monkeypatch, tmp_path):
    parents, handles = setup_model(monkeypatch, tmp_path)
    failure = OSError("cleanup inside unrelated except")
    attempted = []

    def release(handle):
        attempted.append(handle.number)
        if handle.number == 1:
            raise failure

    monkeypatch.setattr(publication, "_release_file_lock", release)
    try:
        raise ValueError("unrelated already handled")
    except ValueError:
        with pytest.raises(OSError) as caught:
            with publication._publication_locks(parents):
                pass
    assert caught.value is failure
    assert attempted == [1, 0]


def test_cleanup_diagnostic_callbacks_cannot_replace_primary(monkeypatch, tmp_path):
    parents, handles = setup_model(monkeypatch, tmp_path)

    class Primary(ValueError):
        def add_note(self, note):
            raise RuntimeError("overridden diagnostic callback")

    class Cleanup(OSError):
        def __str__(self):
            raise RuntimeError("broken diagnostic formatting")

    primary = Primary("original callback")

    def release(handle):
        raise Cleanup()

    monkeypatch.setattr(publication, "_release_file_lock", release)
    with pytest.raises(Primary) as caught:
        with publication._publication_locks(parents):
            raise primary
    assert caught.value is primary and len(primary.__notes__) == 2


class BrokenDiagnosticError(OSError):
    def __str__(self):
        raise RuntimeError("broken exception formatter")


def actual_pairs(tmp_path):
    pairs = []
    for name in ("one.bin", "two.bin"):
        final = tmp_path / name
        final.write_bytes(b"old")
        staged = publication.staged_output_path(final)
        staged.write_bytes(b"new")
        pairs.append((staged, final))
    return pairs


def test_actual_rollback_diagnostic_preserves_primary(monkeypatch, tmp_path):
    primary = ValueError("original transaction failure")
    original = publication._write_journal_copies

    def fail_write(journal):
        original(journal)
        raise primary

    def fail_recovery(paths):
        raise BrokenDiagnosticError()

    monkeypatch.setattr(publication, "_write_journal_copies", fail_write)
    monkeypatch.setattr(publication, "_recover_transaction", fail_recovery)
    with pytest.raises(ValueError) as caught:
        publication.publish_validated_outputs(actual_pairs(tmp_path))
    assert caught.value is primary
    assert primary.__notes__ == [
        "artifact publication rollback recovery failed: BrokenDiagnosticError"
    ]
    assert (tmp_path / "one.bin").read_bytes() == b"old"
    assert (tmp_path / "two.bin").read_bytes() == b"old"


def test_actual_committed_cleanup_diagnostic_cannot_abort_commit(monkeypatch, tmp_path):
    original = publication._recover_transaction

    def recover_then_fault(paths):
        original(paths)
        raise BrokenDiagnosticError()

    monkeypatch.setattr(publication, "_recover_transaction", recover_then_fault)
    with pytest.warns(RuntimeWarning, match="BrokenDiagnosticError"):
        publication.publish_validated_outputs(actual_pairs(tmp_path))
    assert (tmp_path / "one.bin").read_bytes() == b"new"
    assert (tmp_path / "two.bin").read_bytes() == b"new"


def test_discard_stage_diagnostic_does_not_raise_formatter(monkeypatch, tmp_path):
    def fail_unlink(path):
        raise BrokenDiagnosticError()

    monkeypatch.setattr(publication, "_unlink_backup", fail_unlink)
    with pytest.warns(RuntimeWarning, match="BrokenDiagnosticError"):
        publication.discard_staged_output(tmp_path / "owned-stage")


def test_invalid_journal_diagnostic_retains_cause(monkeypatch, tmp_path):
    path = tmp_path / "journal.json"
    path.write_text("{}", encoding="utf-8")
    failure = BrokenDiagnosticError()

    def fail_load(text):
        raise failure

    monkeypatch.setattr(publication, "loads_exact", fail_load)
    with pytest.raises(OSError, match="BrokenDiagnosticError") as caught:
        publication._load_journal(path)
    assert caught.value.__cause__ is failure


@pytest.mark.parametrize("renumbered", [False, True])
def test_legacy_publication_journal_migrates_only_witnessed_members(
    tmp_path, renumbered
):
    import json

    final = tmp_path / "output"
    final.write_bytes(b"previous generation")
    staged = publication.staged_output_path(final)
    staged.write_bytes(b"new generation")
    backup = publication.staged_output_path(final, purpose="backup", suffix=".old")
    transaction = "a" * 32
    journal = tmp_path / f".molt-artifact-publication-{transaction}.json"

    def old_identity(path):
        metadata = path.stat()
        return [metadata.st_dev + int(renumbered), metadata.st_ino]

    payload = {
        "schema": "molt.artifact-publication.v2",
        "transaction_id": transaction,
        "state": "prepared",
        "journals": [str(journal)],
        "removals": [],
        "replacements": [
            {
                "staged": str(staged),
                "final": str(final),
                "backup": str(backup),
                "had_final": True,
                "prior_identity": old_identity(final),
                "installed_identity": old_identity(staged),
            }
        ],
    }
    journal.write_text(json.dumps(payload), encoding="utf-8")
    # Replay a lost older publisher after replacing its output but before
    # writing the durable commit point. The prior generation lives only in its
    # backup; deleting or ignoring this journal would strand that generation.
    final.replace(backup)
    staged.replace(final)
    if renumbered:
        with pytest.raises(
            publication.JournalRecoveryRefused, match="identity-unverifiable"
        ) as caught:
            with publication.publication_locks([final]):
                pass
        assert caught.value.state is publication.JournalState.REFUSED
        assert caught.value.journal == journal
        assert str(journal) in caught.value.remedy
        assert "restore" in caught.value.remedy and "retry" in caught.value.remedy
        assert json.loads(journal.read_text(encoding="utf-8")) == payload
        assert backup.read_bytes() == b"previous generation"
        assert final.read_bytes() == b"new generation"
        assert not publication._journal_path(tmp_path, transaction).exists()
    else:
        # The public locking boundary discovers the older namespace, upgrades
        # only exactly witnessed members and rolls back the interrupted write.
        with publication.publication_locks([final]):
            assert final.read_bytes() == b"previous generation"
        assert not journal.exists()
        assert not publication._journal_path(tmp_path, transaction).exists()
        assert not backup.exists() and not staged.exists()


def test_legacy_journal_recovery_reports_current_residue(tmp_path, monkeypatch):
    import json

    final = tmp_path / "output"
    final.write_bytes(b"old generation")
    staged = publication.staged_output_path(final)
    staged.write_bytes(b"new generation")
    backup = publication.staged_output_path(final, purpose="backup", suffix=".old")
    transaction = "b" * 32
    journal = tmp_path / f".molt-artifact-publication-{transaction}.json"
    current = publication._journal_path(tmp_path, transaction)
    payload = {
        "schema": "molt.artifact-publication.v2",
        "transaction_id": transaction,
        "state": "prepared",
        "journals": [str(journal)],
        "removals": [],
        "replacements": [
            {
                "staged": str(staged),
                "final": str(final),
                "backup": str(backup),
                "had_final": True,
                "prior_identity": [final.stat().st_dev, final.stat().st_ino],
                "installed_identity": [staged.stat().st_dev, staged.stat().st_ino],
            }
        ],
    }
    journal.write_text(json.dumps(payload), encoding="utf-8")
    final.replace(backup)
    staged.replace(final)
    write = publication._write_journal

    def interrupt_terminal(path, value):
        if value["state"] == publication.JournalState.ROLLED_BACK.value:
            raise OSError("interrupted terminal publication")
        write(path, value)

    with publication._publication_locks({tmp_path}):
        with monkeypatch.context() as crash:
            crash.setattr(publication, "_write_journal", interrupt_terminal)
            recovered = publication._recover_transaction((journal,))
    assert recovered.state is publication.JournalState.RECOVERED
    assert recovered.retained == (current,)
    assert not journal.exists() and current.exists()
    assert final.read_bytes() == b"old generation"
    with publication.publication_locks([final]):
        assert final.read_bytes() == b"old generation"
    assert not current.exists()
