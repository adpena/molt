from __future__ import annotations

import os
from pathlib import Path
import shutil
import warnings

import pytest
from molt import file_publication
from molt.cli import source_package_seal as seal_api

from molt.cli.source_package_seal import (
    SourcePackageInput,
    SourcePackageSealVerificationError,
    prepare_source_package_seal_commit,
    recover_source_package_seal_commits,
    stage_source_package_seal,
    verify_source_package_seal,
)


def _write(path: Path, content: bytes) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(content)
    return path


def _stage_fixture(root: Path):
    inputs_root = root / "absolute-input-location"
    source = _write(inputs_root / "module.py", b"VALUE = 42\n")
    generated = _write(inputs_root / "generated.c", b"int answer(void){return 42;}\n")
    return stage_source_package_seal(
        root / "transaction",
        [
            SourcePackageInput(source, "package/module.py", "source"),
            SourcePackageInput(generated, "generated/module.c", "generated"),
        ],
    )


@pytest.mark.parametrize("path", ["CON.py", "a:b.py", "pkg/file. ", "pkg/../file.py"])
def test_seal_paths_use_shared_portable_grammar(path: str) -> None:
    with pytest.raises(SourcePackageSealVerificationError, match="portable"):
        seal_api.validate_source_package_relative_path(path, field="fixture")


def test_seal_rejects_unicode_normalized_path_collision(tmp_path: Path) -> None:
    source = _write(tmp_path / "source", b"payload")
    with pytest.raises(
        seal_api.SourcePackageSealError, match="portable case collision"
    ):
        stage_source_package_seal(
            tmp_path / "transaction",
            [
                SourcePackageInput(source, "pkg/caf\u00e9.py", "source"),
                SourcePackageInput(source, "pkg/cafe\u0301.py", "source"),
            ],
        )


def test_seal_identity_is_relocation_and_input_order_invariant(tmp_path: Path) -> None:
    left_root = tmp_path / "left"
    right_root = tmp_path / "right"
    left_source = _write(left_root / "inputs" / "module.py", b"VALUE = 42\n")
    left_generated = _write(left_root / "inputs" / "module.c", b"int value = 42;\n")
    right_source = _write(right_root / "elsewhere" / "module.py", b"VALUE = 42\n")
    right_generated = _write(
        right_root / "elsewhere" / "module.c", b"int value = 42;\n"
    )

    left = stage_source_package_seal(
        left_root / "transaction",
        [
            SourcePackageInput(left_source, "pkg/module.py", "source"),
            SourcePackageInput(left_generated, "generated/module.c", "generated"),
        ],
    )
    right = stage_source_package_seal(
        right_root / "transaction",
        [
            SourcePackageInput(right_generated, "generated/module.c", "generated"),
            SourcePackageInput(right_source, "pkg/module.py", "source"),
        ],
    )

    assert left.seal_sha256 == right.seal_sha256
    assert left.root.name == left.seal_sha256
    assert right.root.name == right.seal_sha256

    relocated = tmp_path / "relocated-without-digest-name"
    shutil.copytree(left.root, relocated)
    verified = verify_source_package_seal(relocated, expected_sha256=left.seal_sha256)
    assert verified.seal_sha256 == left.seal_sha256
    assert verified.files == left.files


def test_content_store_and_repeated_inputs_are_deduplicated(tmp_path: Path) -> None:
    source = _write(tmp_path / "inputs" / "source.py", b"shared bytes\n")
    generated = _write(tmp_path / "inputs" / "generated.py", b"shared bytes\n")
    transaction_root = tmp_path / "transaction"

    seal = stage_source_package_seal(
        transaction_root,
        [
            SourcePackageInput(source, "src/source.py", "source"),
            SourcePackageInput(source, "src/source.py", "source"),
            SourcePackageInput(generated, "generated/output.py", "generated"),
        ],
    )

    blobs = [
        path
        for path in (transaction_root / "blobs" / "sha256").rglob("*")
        if path.is_file()
    ]
    assert len(blobs) == 1
    assert len(seal.files) == 2
    assert {entry.role for entry in seal.files} == {"source", "generated"}
    assert {entry.sha256 for entry in seal.files} == {blobs[0].name}


@pytest.mark.parametrize("damage", ["missing", "unexpected", "mismatched"])
def test_strict_verifier_rejects_payload_drift(tmp_path: Path, damage: str) -> None:
    seal = _stage_fixture(tmp_path)
    damaged = tmp_path / f"damaged-{damage}"
    shutil.copytree(seal.root, damaged)
    payload_file = damaged / "files" / "package" / "module.py"
    if damage == "missing":
        payload_file.unlink()
    elif damage == "unexpected":
        _write(damaged / "files" / "package" / "surprise.py", b"surprise\n")
    else:
        payload_file.write_bytes(b"VALUE = 43\n")

    with pytest.raises(SourcePackageSealVerificationError):
        verify_source_package_seal(damaged, expected_sha256=seal.seal_sha256)


@pytest.mark.parametrize("after_destination_rename", [False, True])
def test_durable_commit_record_recovers_interrupted_publication(
    tmp_path: Path, after_destination_rename: bool
) -> None:
    seal = _stage_fixture(tmp_path)
    transaction_root = tmp_path / "transaction"
    destination = tmp_path / "published" / "package" / "1.0" / "canonical-seal"
    commit = prepare_source_package_seal_commit(transaction_root, seal, destination)
    assert commit.state == "prepared"
    assert commit.record_path.is_file()
    assert commit.candidate_root.is_dir()

    if after_destination_rename:
        os.replace(commit.candidate_root, commit.destination)

    recovered = recover_source_package_seal_commits(transaction_root)
    assert len(recovered) == 1
    assert recovered[0].state == "committed"
    assert recovered[0].destination == destination.resolve()
    assert not recovered[0].candidate_root.exists()
    assert (
        verify_source_package_seal(
            recovered[0].destination, expected_sha256=seal.seal_sha256
        ).seal_sha256
        == seal.seal_sha256
    )

    # Recovery is idempotent once the post-rename record update is durable.
    repeated = recover_source_package_seal_commits(transaction_root)
    assert len(repeated) == 1
    assert repeated[0].state == "committed"


def test_seal_commit_never_replaces_competing_directory(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    seal = _stage_fixture(tmp_path)
    destination = tmp_path / "published"
    commit = prepare_source_package_seal_commit(
        tmp_path / "transaction", seal, destination
    )
    real_publish = seal_api.durable_publish_directory_exclusive

    def compete(staged: Path, target: Path) -> None:
        target.mkdir()
        real_publish(staged, target)

    monkeypatch.setattr(seal_api, "durable_publish_directory_exclusive", compete)
    with pytest.raises(SourcePackageSealVerificationError, match="inventory mismatch"):
        seal_api.commit_source_package_seal(commit)
    assert destination.is_dir() and not list(destination.iterdir())
    assert commit.candidate_root.is_dir()
    assert (
        seal_api.load_source_package_seal_commit(
            commit.transaction_root, commit.record_path
        ).state
        == "prepared"
    )


def test_recovery_validates_all_destination_custody_before_any_mutation(
    tmp_path: Path,
) -> None:
    seal = _stage_fixture(tmp_path)
    transaction_root = tmp_path / "transaction"
    commits = sorted(
        (
            prepare_source_package_seal_commit(transaction_root, seal, tmp_path / name)
            for name in ("published-first", "published-second")
        ),
        key=lambda commit: commit.record_path,
    )
    before = {
        path.relative_to(transaction_root): path.read_bytes()
        for path in transaction_root.rglob("*")
        if path.is_file()
    }

    # The first journal is owned; a later foreign journal must prevent even
    # that otherwise-valid publication from beginning.
    with pytest.raises(
        SourcePackageSealVerificationError, match="outside destination custody"
    ):
        recover_source_package_seal_commits(
            transaction_root, expected_destination=commits[0].destination
        )

    assert all(not commit.destination.exists() for commit in commits)
    assert all(commit.candidate_root.is_dir() for commit in commits)
    assert before == {
        path.relative_to(transaction_root): path.read_bytes()
        for path in transaction_root.rglob("*")
        if path.is_file()
    }


@pytest.mark.parametrize("kind", ["file", "replacement", "directory", "quarantine"])
def test_postcommit_parent_fsync_failure_warns_without_false_rollback(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, kind: str
) -> None:
    if kind == "file" and os.name != "posix":
        pytest.skip("exclusive file parent fsync applies to POSIX hardlink publication")
    staged = tmp_path / "incoming" / "staged"
    destination = tmp_path / "published" / "artifact"
    destination.parent.mkdir()
    if kind in {"file", "replacement"}:
        _write(staged, b"committed payload")
        publish = (
            file_publication.durable_publish_exclusive
            if kind == "file"
            else file_publication.durable_replace
        )
    else:
        _write(staged / "payload", b"committed payload")
        publish = (
            file_publication.durable_publish_directory_exclusive
            if kind == "directory"
            else file_publication.durable_namespace_publish_directory_exclusive
        )
    failed_parents: list[Path] = []
    real_fsync = file_publication.fsync_directory

    def fail_after_commit(path: Path) -> None:
        if destination.exists():
            failed_parents.append(path)
            raise OSError("injected postcommit fsync failure")
        real_fsync(path)

    monkeypatch.setattr(file_publication, "fsync_directory", fail_after_commit)
    with warnings.catch_warnings():
        warnings.simplefilter("error", RuntimeWarning)
        with pytest.warns(RuntimeWarning, match="publication committed.*durability"):
            publish(staged, destination)

    assert not staged.exists()
    payload = (
        destination if kind in {"file", "replacement"} else destination / "payload"
    )
    assert payload.read_bytes() == b"committed payload"
    assert set(failed_parents) == {staged.parent, destination.parent}


def test_seal_commit_does_not_suppress_noncollision_publication_failure(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    seal = _stage_fixture(tmp_path)
    destination = tmp_path / "published"
    commit = prepare_source_package_seal_commit(
        tmp_path / "transaction", seal, destination
    )

    def fail(staged: Path, target: Path) -> None:
        raise OSError("injected durability failure")

    monkeypatch.setattr(seal_api, "durable_publish_directory_exclusive", fail)
    with pytest.raises(OSError, match="injected durability failure"):
        seal_api.commit_source_package_seal(commit)
    assert not destination.exists()
    assert commit.candidate_root.exists()


def test_seal_tree_flush_has_one_owner(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    flushed: list[Path] = []
    real_flush = file_publication._flush_staged_file

    def observe(path: Path) -> int:
        flushed.append(path)
        return real_flush(path)

    monkeypatch.setattr(file_publication, "_flush_staged_file", observe)
    seal = _stage_fixture(tmp_path)
    payload_flushes = [path for path in flushed if "files" in path.parts]
    assert len(payload_flushes) == len(seal.files)
    assert len(set(payload_flushes)) == len(payload_flushes)


def test_exclusive_blob_publication_supports_cross_directory_staging(
    tmp_path: Path,
) -> None:
    staged = _write(tmp_path / "incoming" / "blob", b"payload")
    destination = tmp_path / "store" / "digest"
    destination.parent.mkdir()
    file_publication.durable_publish_exclusive(staged, destination)
    assert not staged.exists()
    assert destination.read_bytes() == b"payload"
    staged.write_bytes(b"rival")
    with pytest.raises(FileExistsError):
        file_publication.durable_publish_exclusive(staged, destination)
    assert staged.read_bytes() == b"rival"
    assert destination.read_bytes() == b"payload"


def test_quarantine_does_not_inspect_corrupt_payloads_or_replace_rival(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    source = tmp_path / "damaged"
    source.mkdir()
    (source / "evidence").write_bytes(b"broken")
    target = tmp_path / "quarantine"

    def reject_flush(root: Path) -> None:
        raise AssertionError("quarantine must not inspect damaged bytes")

    monkeypatch.setattr(file_publication, "_flush_staged_directory_tree", reject_flush)
    file_publication.durable_namespace_publish_directory_exclusive(source, target)
    assert (target / "evidence").read_bytes() == b"broken"
    source.mkdir()
    with pytest.raises(FileExistsError):
        file_publication.durable_namespace_publish_directory_exclusive(source, target)
    assert source.exists()


def test_seal_commit_recovery_reclaims_partially_deleted_candidate(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    seal = _stage_fixture(tmp_path)
    store = tmp_path / "publication"
    destination = tmp_path / "installed"
    commit = prepare_source_package_seal_commit(store, seal, destination)
    # Another publisher installs the identical admitted seal before this commit.
    seal_api._copy_seal_candidate(seal.root, destination, seal.seal_sha256)
    real_remove = file_publication.shutil.rmtree
    with monkeypatch.context() as faults:

        def partial_remove(path, *args, **kwargs):
            if path.parent == commit.candidate_root.parent:
                (path / "source-package-seal.json").unlink()
                raise OSError("injected after candidate manifest unlink")
            return real_remove(path, *args, **kwargs)

        faults.setattr(file_publication.shutil, "rmtree", partial_remove)
        with pytest.raises(file_publication.RetirementError) as caught:
            seal_api.commit_source_package_seal(commit)
    assert not commit.candidate_root.exists()
    assert caught.value.retired_path.exists()
    recovered = recover_source_package_seal_commits(
        store, expected_destination=destination
    )
    assert len(recovered) == 1 and recovered[0].state == "committed"
    assert not caught.value.retired_path.exists()
    assert verify_source_package_seal(destination).seal_sha256 == seal.seal_sha256


@pytest.mark.parametrize("family", ["staging", "copy"])
def test_private_seal_scratch_recovery_owns_retired_names(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, family: str
) -> None:
    seal = _stage_fixture(tmp_path)
    store = tmp_path / "private"
    parent = store / ("staging" if family == "staging" else "commit-candidates")
    root = parent / "old-private-scratch"
    root.mkdir(parents=True)
    (root / "journal").write_bytes(b"old")
    scope = (
        seal_api._STAGING_RETIREMENT_SCOPE
        if family == "staging"
        else seal_api._COPY_RETIREMENT_SCOPE
    )
    with monkeypatch.context() as faults:

        def partial_remove(path):
            (path / "journal").unlink()
            raise OSError("injected physical interruption")

        faults.setattr(file_publication.shutil, "rmtree", partial_remove)
        with pytest.raises(file_publication.RetirementError) as caught:
            file_publication.durable_remove_path(root, retirement_scope=scope)
    live = parent / "new-private-scratch"
    live.mkdir()
    (live / "payload").write_bytes(b"preserve")
    assert recover_source_package_seal_commits(store) == ()
    assert not caught.value.retired_path.exists()
    assert (live / "payload").read_bytes() == b"preserve"
    assert verify_source_package_seal(seal.root).seal_sha256 == seal.seal_sha256


def test_copy_failure_keeps_primary_and_retirement_failure_then_reclaims(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    seal = _stage_fixture(tmp_path)
    candidate = tmp_path / "bundle" / "candidate"
    primary = OSError("injected payload copy failure")
    with monkeypatch.context() as faults:

        def fail_copy(*_args, **_kwargs):
            raise primary

        def partial_remove(path):
            (path / "files").rmdir()
            raise OSError("injected scratch removal failure")

        faults.setattr(seal_api, "_copy_staged_file", fail_copy)
        faults.setattr(file_publication.shutil, "rmtree", partial_remove)
        with pytest.raises(seal_api.SourcePackageSealCleanupError) as caught:
            seal_api._copy_seal_candidate(seal.root, candidate, seal.seal_sha256)
    assert caught.value.primary_error is primary
    assert caught.value.__cause__ is primary
    assert isinstance(caught.value.cleanup_error, file_publication.RetirementError)
    retired = caught.value.cleanup_error.retired_path
    assert retired.exists() and not candidate.exists()
    seal_api._copy_seal_candidate(seal.root, candidate, seal.seal_sha256)
    assert not retired.exists()
    assert verify_source_package_seal(candidate).seal_sha256 == seal.seal_sha256
