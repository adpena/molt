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
    path.write_text("{}")
    failure = BrokenDiagnosticError()

    def fail_load(text):
        raise failure

    monkeypatch.setattr(publication, "loads_exact", fail_load)
    with pytest.raises(OSError, match="BrokenDiagnosticError") as caught:
        publication._load_journal(path)
    assert caught.value.__cause__ is failure
