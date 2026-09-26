from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor
import stat
from pathlib import Path
from threading import Event

import pytest
from molt import artifact_publication as module


@pytest.mark.parametrize("readonly", [False, True])
def test_publish_validated_outputs_replaces_only_complete_staged_set(
    tmp_path: Path,
    readonly: bool,
) -> None:
    final_a = tmp_path / "final-a.bin"
    final_b = tmp_path / "nested" / "final-b.bin"
    final_a.write_bytes(b"old-a")
    final_b.parent.mkdir()
    final_b.write_bytes(b"old-b")
    staged_a = module.staged_output_path(final_a)
    staged_b = module.staged_output_path(final_b)
    staged_a.write_bytes(b"new-a")
    staged_b.write_bytes(b"new-b")
    if readonly:
        for path in (final_a, final_b, staged_a, staged_b):
            path.chmod(stat.S_IREAD)

    module.publish_validated_outputs([(staged_a, final_a), (staged_b, final_b)])

    assert final_a.read_bytes() == b"new-a"
    assert final_b.read_bytes() == b"new-b"
    assert not staged_a.exists()
    assert not staged_b.exists()
    assert not list(tmp_path.rglob("*.old"))
    assert not list(tmp_path.rglob(".molt-artifact-publication-*.json"))
    if readonly:
        assert not final_a.stat().st_mode & stat.S_IWRITE
        assert not final_b.stat().st_mode & stat.S_IWRITE


def test_staging_and_backup_components_are_bounded_independently_of_final_name(
    tmp_path: Path,
) -> None:
    final = tmp_path / (("long-output-name-" * 13) + ".wasm")

    staged = module.staged_output_path(
        final,
        purpose="wasm-opt",
        suffix=".wasm",
    )
    backup = module.staged_output_path(
        final,
        purpose="backup",
        suffix=".old",
    )

    assert staged.parent == final.parent
    assert backup.parent == final.parent
    assert staged.name.startswith(".molt-wasm-opt-")
    assert backup.name.startswith(".molt-backup-")
    assert staged.suffix == ".wasm"
    assert backup.suffix == ".old"
    assert len(staged.name) <= 80
    assert len(backup.name) <= 80
    assert final.name not in staged.name
    assert final.name not in backup.name


def test_publish_validated_outputs_missing_staged_preserves_old_final_bytes(
    tmp_path: Path,
) -> None:
    final_a = tmp_path / "final-a.bin"
    final_b = tmp_path / "final-b.bin"
    staged_a = module.staged_output_path(final_a)
    missing_staged_b = module.staged_output_path(final_b)
    final_a.write_bytes(b"old-a")
    final_b.write_bytes(b"old-b")
    staged_a.write_bytes(b"new-a")

    with pytest.raises(FileNotFoundError, match="staged artifact missing"):
        module.publish_validated_outputs(
            [(staged_a, final_a), (missing_staged_b, final_b)]
        )

    assert final_a.read_bytes() == b"old-a"
    assert final_b.read_bytes() == b"old-b"


def test_publish_validated_outputs_rolls_back_after_partial_replace_failure(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    final_a = tmp_path / "final-a.bin"
    final_b = tmp_path / "final-b.bin"
    staged_a = module.staged_output_path(final_a)
    staged_b = module.staged_output_path(final_b)
    final_a.write_bytes(b"old-a")
    final_b.write_bytes(b"old-b")
    staged_a.write_bytes(b"new-a")
    staged_b.write_bytes(b"new-b")
    original_replace = module._durable_replace
    first_final_replaced = False

    def failing_replace(src: Path, dst: Path) -> None:
        nonlocal first_final_replaced
        src_path = Path(src)
        dst_path = Path(dst)
        if src_path == staged_b and dst_path == final_b:
            assert first_final_replaced
            raise OSError("simulated second final replace failure")
        original_replace(src, dst)
        if src_path == staged_a and dst_path == final_a:
            first_final_replaced = True

    monkeypatch.setattr(module, "_durable_replace", failing_replace)

    with pytest.raises(OSError, match="simulated second final replace failure"):
        module.publish_validated_outputs([(staged_a, final_a), (staged_b, final_b)])

    assert final_a.read_bytes() == b"old-a"
    assert final_b.read_bytes() == b"old-b"
    assert not list(tmp_path.glob(".*.old"))


@pytest.mark.parametrize("mode", [0o640, stat.S_IREAD])
def test_atomic_copy_file_copies_bytes_and_mode_through_publish_path(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, mode: int
) -> None:
    src = tmp_path / "source.bin"
    dst = tmp_path / "out" / "final.bin"
    src.write_bytes(b"\x00molt-bytes\xff")
    src.chmod(mode)
    expected_mode = stat.S_IMODE(src.stat().st_mode)
    original_publish = module.publish_validated_outputs
    published_pairs: list[list[tuple[Path, Path]]] = []

    def spy_publish(pairs: list[tuple[Path, Path]]) -> None:
        normalized = [(Path(staged), Path(final)) for staged, final in pairs]
        published_pairs.append(normalized)
        original_publish(pairs)

    monkeypatch.setattr(module, "publish_validated_outputs", spy_publish)

    module.atomic_copy_file(src, dst)

    assert dst.read_bytes() == b"\x00molt-bytes\xff"
    assert stat.S_IMODE(dst.stat().st_mode) == expected_mode
    assert len(published_pairs) == 1
    assert len(published_pairs[0]) == 1
    staged, final = published_pairs[0][0]
    assert final == dst
    assert staged.parent == dst.parent
    assert staged != dst
    assert not staged.exists()


def test_abandoned_readonly_copy_reclaims_private_stage(tmp_path: Path) -> None:
    source = tmp_path / "source"
    source.write_bytes(b"readonly")
    source.chmod(stat.S_IREAD)
    final = tmp_path / "final"
    with pytest.raises(ValueError, match="candidate rejected"):
        with module.staged_copy_file(source, final) as staged:
            assert not staged.stat().st_mode & stat.S_IWRITE
            raise ValueError("candidate rejected")
    assert not staged.exists()
    assert not final.exists()
    assert source.read_bytes() == b"readonly"
    assert not source.stat().st_mode & stat.S_IWRITE


def test_atomic_write_bytes_preserves_existing_bytes_when_replace_fails(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    final = tmp_path / "artifact.bin"
    final.write_bytes(b"old-bytes")
    original_replace = module._durable_replace

    def failing_final_replace(src: Path, dst: Path) -> None:
        src_path = Path(src)
        dst_path = Path(dst)
        if src_path.suffix == ".tmp" and dst_path == final:
            raise OSError("simulated atomic write replace failure")
        original_replace(src, dst)

    monkeypatch.setattr(module, "_durable_replace", failing_final_replace)

    with pytest.raises(OSError, match="simulated atomic write replace failure"):
        module.atomic_write_bytes(final, b"new-bytes")

    assert final.read_bytes() == b"old-bytes"
    assert not list(tmp_path.glob(".*.tmp"))
    assert not list(tmp_path.glob(".*.old"))


def test_atomic_write_text_encodes_with_requested_encoding(tmp_path: Path) -> None:
    final = tmp_path / "artifact.txt"

    module.atomic_write_text(final, "alpha\nbeta", encoding="utf-16le")

    assert final.read_bytes() == "alpha\nbeta".encode("utf-16le")


def test_atomic_write_json_writes_sorted_indented_text_with_trailing_newline(
    tmp_path: Path,
) -> None:
    final = tmp_path / "payload.json"

    module.atomic_write_json(
        final,
        {"z": 2, "a": {"c": 3, "b": 1}},
        indent=4,
        sort_keys=True,
    )

    assert final.read_text(encoding="utf-8") == (
        '{\n    "a": {\n        "b": 1,\n        "c": 3\n    },\n    "z": 2\n}\n'
    )


def test_rejects_unowned_stage_and_publication_authority_destinations(
    tmp_path: Path,
) -> None:
    final = tmp_path / "final.bin"
    unowned = tmp_path / ".caller-private.tmp"
    unowned.write_bytes(b"new")
    with pytest.raises(ValueError, match="owned staging namespace"):
        module.publish_validated_outputs([(unowned, final)])

    reserved = (
        tmp_path / ".molt-artifact-publication.lock",
        tmp_path / ".MOLT-ARTIFACT-PUBLICATION.LOCK",
        module._journal_path(tmp_path, "a" * 32),
        tmp_path / f".molt-artifact-publication-{'a' * 32}.json.stage-{'b' * 32}.tmp",
    )
    for path in reserved:
        staged = module.staged_output_path(path)
        staged.write_bytes(b"new")
        with pytest.raises(ValueError, match="reserved for publication custody"):
            module.publish_validated_outputs([(staged, path)])
        with pytest.raises(ValueError, match="reserved for publication custody"):
            module.publish_validated_outputs([], removals=(path,))
    assert unowned.read_bytes() == b"new"
    assert not final.exists()


def test_rejects_stage_overlapping_another_destination(tmp_path: Path) -> None:
    final = tmp_path / "final.bin"
    staged = module.staged_output_path(final)
    staged.write_bytes(b"new")
    with pytest.raises(ValueError, match="overlaps a publication destination"):
        module.publish_validated_outputs([(staged, final)], removals=(staged,))
    assert staged.read_bytes() == b"new"


def test_committed_partial_journal_copy_preserves_new_generation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    final_a = tmp_path / "a" / "final.bin"
    final_b = tmp_path / "b" / "final.bin"
    staged_a = module.staged_output_path(final_a)
    staged_b = module.staged_output_path(final_b)
    final_a.write_bytes(b"old-a")
    final_b.write_bytes(b"old-b")
    staged_a.write_bytes(b"new-a")
    staged_b.write_bytes(b"new-b")
    original_write = module._write_journal
    committed_copies = 0
    failed_once = False

    def fail_second_committed_copy(path: Path, payload: dict[str, object]) -> None:
        nonlocal committed_copies, failed_once
        if payload["state"] == "committed":
            if committed_copies and not failed_once:
                failed_once = True
                raise OSError("simulated committed journal copy loss")
            committed_copies += 1
        original_write(path, payload)

    monkeypatch.setattr(module, "_write_journal", fail_second_committed_copy)
    retained = module.publish_validated_outputs(
        [(staged_a, final_a), (staged_b, final_b)]
    )

    assert retained == ()
    assert committed_copies == 2
    assert failed_once
    assert final_a.read_bytes() == b"new-a"
    assert final_b.read_bytes() == b"new-b"
    assert not list(tmp_path.rglob(".molt-artifact-publication-*.json"))


def test_committed_copy_survives_propagation_and_cleanup_interruptions(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    final_a = tmp_path / "a" / "new.bin"
    final_b = tmp_path / "b" / "new.bin"
    staged_a = module.staged_output_path(final_a)
    staged_b = module.staged_output_path(final_b)
    staged_a.write_bytes(b"new-a")
    staged_b.write_bytes(b"new-b")
    original_write = module._write_journal

    def fail_second_committed_copy(path: Path, payload: dict[str, object]) -> None:
        if payload["state"] == "committed" and path.parent == final_b.parent:
            raise OSError("simulated partial committed-copy write")
        original_write(path, payload)

    with monkeypatch.context() as initial:
        initial.setattr(module, "_write_journal", fail_second_committed_copy)
        with pytest.warns(RuntimeWarning, match="journal-owned cleanup residue"):
            retained = module.publish_validated_outputs(
                [(staged_a, final_a), (staged_b, final_b)]
            )
    assert len(retained) == 2
    assert final_a.read_bytes() == b"new-a"
    assert final_b.read_bytes() == b"new-b"
    journal_a = next(final_a.parent.glob(".molt-artifact-publication-*.json"))
    journal_b = next(final_b.parent.glob(".molt-artifact-publication-*.json"))
    assert module._load_journal(journal_a)["state"] == "committed"
    assert module._load_journal(journal_b)["state"] == "prepared"

    original_unlink = module._unlink_backup

    def interrupt_second_journal_cleanup(path: Path) -> None:
        if path == journal_b:
            raise OSError("simulated cleanup interruption")
        original_unlink(path)

    with monkeypatch.context() as cleanup:
        cleanup.setattr(module, "_unlink_backup", interrupt_second_journal_cleanup)
        with pytest.raises(OSError, match="retained cleanup residue"):
            with module.publication_locks([final_a]):
                pass

    assert not journal_a.exists()
    assert module._load_journal(journal_b)["state"] == "committed"
    with module.publication_locks([final_b]):
        assert final_a.read_bytes() == b"new-a"
        assert final_b.read_bytes() == b"new-b"
    assert not journal_b.exists()


def test_next_publication_recovers_interrupted_cross_directory_transaction(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    final_a = tmp_path / "final-a.bin"
    final_b = tmp_path / "nested" / "final-b.bin"
    staged_a = module.staged_output_path(final_a)
    staged_b = module.staged_output_path(final_b)
    final_a.write_bytes(b"old-a")
    final_b.write_bytes(b"old-b")
    staged_a.write_bytes(b"new-a")
    staged_b.write_bytes(b"new-b")
    original_replace = module._durable_replace

    def interrupt_after_first_replace(src: Path, dst: Path) -> None:
        if Path(src) == staged_b and Path(dst) == final_b:
            raise OSError("simulated process loss")
        original_replace(Path(src), Path(dst))

    with monkeypatch.context() as crash:
        crash.setattr(module, "_durable_replace", interrupt_after_first_replace)
        crash.setattr(
            module,
            "_recover_transaction",
            lambda _journals: module._TransactionRecovery(committed=False, retained=()),
        )
        with pytest.raises(OSError, match="simulated process loss"):
            module.publish_validated_outputs([(staged_a, final_a), (staged_b, final_b)])

    assert final_a.read_bytes() == b"new-a"
    assert not final_b.exists()
    assert list(tmp_path.glob(".molt-artifact-publication-*.json"))
    assert list(final_b.parent.glob(".molt-artifact-publication-*.json"))

    with module.publication_locks([final_a]):
        assert final_a.read_bytes() == b"old-a"
        assert final_b.read_bytes() == b"old-b"

    fresh_final = tmp_path / "fresh.bin"
    fresh_stage = module.staged_output_path(fresh_final)
    fresh_stage.write_bytes(b"fresh")
    module.publish_validated_outputs([(fresh_stage, fresh_final)])
    assert fresh_final.read_bytes() == b"fresh"
    assert not list(tmp_path.rglob(".molt-artifact-publication-*.json"))


@pytest.mark.parametrize("interrupt", ["propagation", "cleanup"])
def test_aborted_orphan_never_rolls_back_a_later_generation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, interrupt: str
) -> None:
    finals = [tmp_path / name / "app.bin" for name in ("a", "b")]
    stages = [module.staged_output_path(path) for path in finals]
    for stage in stages:
        stage.write_bytes(b"aborted")
    original_replace = module._durable_replace
    original_write = module._write_journal
    original_unlink = module._unlink_backup

    def fail_second_output(source: Path, destination: Path) -> None:
        if source == stages[1]:
            raise OSError("interrupted publisher")
        original_replace(source, destination)

    def fail_terminal_propagation(path: Path, payload: dict) -> None:
        if payload["state"] == "aborted" and path.parent == finals[1].parent:
            raise OSError("interrupted terminal propagation")
        original_write(path, payload)

    def fail_journal_cleanup(path: Path) -> None:
        if (
            path.name.startswith(".molt-artifact-publication-")
            and path.suffix == ".json"
            and path.parent == finals[1].parent
        ):
            raise OSError("interrupted journal cleanup")
        original_unlink(path)

    with monkeypatch.context() as crash:
        crash.setattr(module, "_durable_replace", fail_second_output)
        if interrupt == "propagation":
            crash.setattr(module, "_write_journal", fail_terminal_propagation)
        else:
            crash.setattr(module, "_unlink_backup", fail_journal_cleanup)
        with pytest.raises(OSError, match="interrupted publisher"):
            module.publish_validated_outputs(list(zip(stages, finals)))
    assert all(not path.exists() for path in finals)
    if interrupt == "propagation":
        with monkeypatch.context() as crash:
            crash.setattr(module, "_unlink_backup", fail_journal_cleanup)
            with pytest.raises(OSError, match="retained cleanup residue"):
                with module.publication_locks([finals[0]]):
                    pass
    assert not list(finals[0].parent.glob(".molt-artifact-publication-*.json"))
    orphan = next(finals[1].parent.glob(".molt-artifact-publication-*.json"))
    assert module._load_journal(orphan)["state"] == "aborted"
    module.atomic_write_bytes(finals[0], b"later generation")
    with module.publication_locks([finals[1]]):
        assert finals[0].read_bytes() == b"later generation"
    assert not orphan.exists()


def test_prepared_recovery_preserves_an_unrelated_replacement(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    finals = [tmp_path / name for name in ("a.bin", "b.bin")]
    stages = [module.staged_output_path(path) for path in finals]
    for stage in stages:
        stage.write_bytes(b"interrupted")
    original_replace = module._durable_replace

    def interrupt(source: Path, destination: Path) -> None:
        if source == stages[1]:
            raise OSError("publisher lost")
        original_replace(source, destination)

    with monkeypatch.context() as crash:
        crash.setattr(module, "_durable_replace", interrupt)
        crash.setattr(
            module,
            "_recover_transaction",
            lambda _: module._TransactionRecovery(False, ()),
        )
        with pytest.raises(OSError, match="publisher lost"):
            module.publish_validated_outputs(list(zip(stages, finals)))
    replacement = tmp_path / "foreign.bin"
    replacement.write_bytes(b"unrelated replacement")
    replacement.replace(finals[0])
    with pytest.raises(OSError, match="another generation"):
        with module.publication_locks([finals[0]]):
            pass
    assert finals[0].read_bytes() == b"unrelated replacement"
    assert list(tmp_path.glob(".molt-artifact-publication-*.json"))


@pytest.mark.parametrize("mutation", ["contents", "replace", "add", "publish"])
def test_payload_snapshot_rejects_changed_generation(
    tmp_path: Path, mutation: str
) -> None:
    root = tmp_path / "package"
    root.mkdir()
    source = root / "a.py"
    source.write_bytes(b"before")
    with pytest.raises(ValueError, match="changed"):
        with module.publication_payload_snapshot([root]) as snapshot:
            assert snapshot[root] == (source,)
            if mutation == "contents":
                source.write_bytes(b"after!")
            elif mutation == "replace":
                replacement = tmp_path / "replacement"
                replacement.write_bytes(b"before")
                replacement.replace(source)
            elif mutation == "add":
                (root / "b.py").write_bytes(b"new")
            else:
                module.atomic_write_bytes(source, b"after!")


def test_payload_snapshot_omits_private_state_without_modifying_plain_sources(
    tmp_path: Path,
) -> None:
    root = tmp_path / "package"
    root.mkdir()
    source = root / "a.py"
    source.write_bytes(b"payload")
    stage = module.staged_output_path(source)
    stage.write_bytes(b"private bytes")
    generation = module.staged_output_path(root / "manifest.json", purpose="generation")
    generation.mkdir()
    (generation / "app.wasm").write_bytes(b"unfinished")
    with module.publication_payload_snapshot([root]) as snapshot:
        assert snapshot == {root: (source,)}
    assert not (root / module._PUBLICATION_LOCK_NAME).exists()


def test_committed_cleanup_warning_does_not_reverse_generation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    final = tmp_path / "final.bin"
    obsolete = tmp_path / "obsolete.bin"
    staged = module.staged_output_path(final)
    obsolete.write_bytes(b"obsolete")
    staged.write_bytes(b"new")
    original_unlink = module._unlink_backup

    def retain_backup(path: Path) -> None:
        if path.suffix == ".old":
            raise PermissionError("simulated sharing violation")
        original_unlink(path)

    monkeypatch.setattr(module, "_unlink_backup", retain_backup)
    with pytest.warns(RuntimeWarning, match="journal-owned cleanup residue"):
        retained = module.publish_validated_outputs(
            [(staged, final)], removals=(obsolete,)
        )
    assert final.read_bytes() == b"new"
    assert not obsolete.exists()
    assert len(retained) == 1
    assert retained[0].suffix == ".old"

    monkeypatch.setattr(module, "_unlink_backup", original_unlink)
    with module.publication_locks([final]):
        assert final.read_bytes() == b"new"
    assert not list(tmp_path.glob("*.old"))
    assert not list(tmp_path.glob(".molt-artifact-publication-*.json"))


def test_overlapping_publication_sets_share_parent_lock(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    final_a = tmp_path / "a" / "first.bin"
    final_b = tmp_path / "b" / "second.bin"
    staged_b = module.staged_output_path(final_b)
    staged_b.write_bytes(b"second")
    final_a.parent.mkdir(exist_ok=True)
    attempted = Event()
    original_acquire = module._acquire_file_lock
    lock_b = module._publication_lock_path(final_b.parent)

    def observe_acquire(path: Path, **kwargs: object):
        if path == lock_b:
            attempted.set()
        return original_acquire(path, **kwargs)

    with ThreadPoolExecutor(max_workers=1) as pool:
        with module.publication_locks([final_a, final_b]):
            monkeypatch.setattr(module, "_acquire_file_lock", observe_acquire)
            future = pool.submit(
                module.publish_validated_outputs, [(staged_b, final_b)]
            )
            assert attempted.wait(timeout=5), "overlapping publisher did not reach lock"
            assert not future.done()
        assert future.result(timeout=5) == ()
    assert final_b.read_bytes() == b"second"


def test_publication_lock_rejects_indirect_leaf(tmp_path: Path) -> None:
    final = tmp_path / "final.bin"
    lock = module._publication_lock_path(tmp_path)
    target = tmp_path / "outside.bin"
    target.write_bytes(b"outside")
    try:
        lock.symlink_to(target)
    except OSError as exc:
        pytest.skip(f"symlinks unavailable on this host: {exc}")
    with pytest.raises(ValueError, match="lock must not be indirect"):
        with module.publication_locks([final]):
            pass
    assert target.read_bytes() == b"outside"
