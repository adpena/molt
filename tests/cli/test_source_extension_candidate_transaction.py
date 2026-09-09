from __future__ import annotations

import json
from pathlib import Path

import pytest

from molt.file_locks import _acquire_file_lock, _release_file_lock
from molt.cli import source_extension_candidate_transaction as transaction_authority
from molt.cli.source_package_seal import (
    SourcePackageInput,
    _copy_seal_candidate,
    stage_source_package_seal,
    verify_source_package_seal,
)
from molt.exact_json import write_exact
from molt.file_hashing import _sha256_file
from molt.cli.source_extension_candidate_transaction import (
    SourceExtensionCandidateTransactionError,
    begin_source_extension_candidate_transaction,
    fail_source_extension_candidate_transaction,
    recover_and_prune_source_extension_candidate_transactions,
    source_extension_candidate_transaction_custody,
)


def _held_custody(output: Path):
    lock_path = output.parent / f".{output.name}.candidate.lock"
    handle = _acquire_file_lock(
        lock_path,
        timeout_s=1.0,
        timeout_message="fixture candidate lock unavailable",
    )
    return handle, source_extension_candidate_transaction_custody(output, handle)


def _begin(transaction: Path, custody, *, now_ns: int = 1) -> None:
    transaction.mkdir()
    begin_source_extension_candidate_transaction(
        transaction,
        custody=custody,
        package="numpy",
        package_version="2.5.1",
        module_set="pact-witness",
        cpython="3.12",
        abi_tier="cpython-abi",
        target_triple="wasm32-wasip1",
        now_ns=now_ns,
    )


def test_candidate_transaction_recovery_marks_interrupted_then_prunes_by_age(
    tmp_path: Path,
) -> None:
    output = (tmp_path / "numpy-candidate").resolve()
    transaction = tmp_path / ".numpy-candidate.attest-crashed"
    handle, custody = _held_custody(output)
    try:
        _begin(transaction, custody)
        assert (
            recover_and_prune_source_extension_candidate_transactions(
                custody=custody,
                now_ns=10,
                failed_retention_seconds=1,
            )
            == ()
        )
        record = json.loads(
            (transaction / "candidate-transaction.json").read_text(encoding="utf-8")
        )
        assert record["state"] == "interrupted"
        assert recover_and_prune_source_extension_candidate_transactions(
            custody=custody,
            now_ns=1_000_000_011,
            failed_retention_seconds=1,
        ) == (transaction.resolve(),)
        assert not transaction.exists()
    finally:
        _release_file_lock(handle)


def test_candidate_transaction_pruning_preserves_malformed_or_untyped_evidence(
    tmp_path: Path,
) -> None:
    output = (tmp_path / "numpy-candidate").resolve()
    ambiguous = tmp_path / ".numpy-candidate.attest-legacy"
    ambiguous.mkdir()
    (ambiguous / "candidate-transaction.json").write_text("{}\n", encoding="utf-8")
    handle, custody = _held_custody(output)
    try:
        with pytest.raises(
            SourceExtensionCandidateTransactionError, match="invalid schema"
        ):
            recover_and_prune_source_extension_candidate_transactions(
                custody=custody,
                now_ns=10**20,
                failed_retention_seconds=0,
            )
        assert ambiguous.exists()
    finally:
        _release_file_lock(handle)


def test_candidate_transaction_requires_live_exact_output_lock(tmp_path: Path) -> None:
    output = (tmp_path / "numpy-candidate").resolve()
    transaction = tmp_path / ".numpy-candidate.attest-failed"
    handle, custody = _held_custody(output)
    _begin(transaction, custody)
    fail_source_extension_candidate_transaction(
        transaction,
        custody=custody,
        error="fixture failure",
        now_ns=2,
    )
    _release_file_lock(handle)

    with pytest.raises(
        SourceExtensionCandidateTransactionError, match="live exclusive"
    ):
        recover_and_prune_source_extension_candidate_transactions(
            custody=custody,
            now_ns=10**20,
            failed_retention_seconds=0,
        )


def _stage_bundle(root: Path) -> tuple[str, str]:
    source = root / "payload.bin"
    source.write_bytes(b"exact prepared candidate bytes")
    seal = stage_source_package_seal(
        root / "package-store",
        (SourcePackageInput(source, "demo/payload.bin", "package-file"),),
    )
    bundle = root / transaction_authority.CANDIDATE_BUNDLE_DIRECTORY
    bundle.mkdir()
    _copy_seal_candidate(
        seal.root,
        bundle / transaction_authority.CANDIDATE_SEAL_DIRECTORY,
        seal.seal_sha256,
    )
    report = bundle / transaction_authority.CANDIDATE_ATTESTATION_NAME
    write_exact(report, {"fixture": "validated candidate report"})
    return seal.seal_sha256, _sha256_file(report)


@pytest.mark.parametrize(
    "failure_boundary", ["before-rename", "after-rename", "cleanup"]
)
def test_candidate_commit_journal_survives_every_namespace_boundary(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    failure_boundary: str,
) -> None:
    output = tmp_path / "candidate"
    root = tmp_path / ".candidate.attest-test"
    handle, custody = _held_custody(output)
    try:
        _begin(root, custody)
        seal_hash, report_hash = _stage_bundle(root)
        with monkeypatch.context() as faults:
            if failure_boundary == "before-rename":

                def fail_rename(*_args):
                    raise OSError("injected before rename")

                faults.setattr(
                    transaction_authority,
                    "durable_publish_directory_exclusive",
                    fail_rename,
                )
            elif failure_boundary == "after-rename":
                real_write = transaction_authority._write_record

                def fail_committed_write(path, record, **kwargs):
                    if record["state"] == "committed":
                        raise OSError("injected after rename")
                    return real_write(path, record, **kwargs)

                faults.setattr(
                    transaction_authority, "_write_record", fail_committed_write
                )
            else:

                def fail_cleanup(*_args):
                    raise OSError("injected cleanup")

                faults.setattr(
                    transaction_authority, "durable_remove_path", fail_cleanup
                )
            with pytest.raises(OSError, match="injected"):
                transaction_authority.commit_source_extension_candidate_transaction(
                    root,
                    custody=custody,
                    seal_sha256=seal_hash,
                    report_sha256=report_hash,
                )
                transaction_authority.complete_source_extension_candidate_transaction(
                    root, custody=custody
                )
        assert (root / "candidate-transaction.json").is_file()
        assert output.exists() is (failure_boundary != "before-rename")
        assert recover_and_prune_source_extension_candidate_transactions(
            custody=custody
        ) == (root,)
        assert not root.exists()
        assert (
            verify_source_package_seal(
                output / transaction_authority.CANDIDATE_SEAL_DIRECTORY
            ).seal_sha256
            == seal_hash
        )
    finally:
        _release_file_lock(handle)


def test_candidate_commit_refuses_destination_collision_and_retains_journal(
    tmp_path: Path,
) -> None:
    output = tmp_path / "candidate"
    root = tmp_path / ".candidate.attest-test"
    handle, custody = _held_custody(output)
    try:
        _begin(root, custody)
        seal_hash, report_hash = _stage_bundle(root)
        output.mkdir()
        (output / "incumbent").write_bytes(b"preserve")
        with pytest.raises(FileExistsError):
            transaction_authority.commit_source_extension_candidate_transaction(
                root,
                custody=custody,
                seal_sha256=seal_hash,
                report_sha256=report_hash,
            )
        assert (output / "incumbent").read_bytes() == b"preserve"
        assert (root / "candidate-transaction.json").is_file()
        with pytest.raises(FileExistsError):
            recover_and_prune_source_extension_candidate_transactions(custody=custody)
    finally:
        _release_file_lock(handle)


@pytest.mark.parametrize("field", ["schema_version", "created_at_ns", "updated_at_ns"])
def test_candidate_journal_rejects_boolean_integer_fields(
    tmp_path: Path, field: str
) -> None:
    output = tmp_path / "candidate"
    root = tmp_path / ".candidate.attest-test"
    handle, custody = _held_custody(output)
    try:
        _begin(root, custody)
        journal = root / "candidate-transaction.json"
        record = json.loads(journal.read_text())
        record[field] = True
        write_exact(journal, record)
        with pytest.raises(
            SourceExtensionCandidateTransactionError, match="invalid values"
        ):
            recover_and_prune_source_extension_candidate_transactions(custody=custody)
        assert root.exists()
    finally:
        _release_file_lock(handle)


def test_failure_recording_preserves_prepared_commit_intent(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    output = tmp_path / "candidate"
    root = tmp_path / ".candidate.attest-test"
    handle, custody = _held_custody(output)
    try:
        _begin(root, custody)
        seal_hash, report_hash = _stage_bundle(root)
        with monkeypatch.context() as faults:

            def fail_rename(*_args):
                raise OSError("interrupted")

            faults.setattr(
                transaction_authority,
                "durable_publish_directory_exclusive",
                fail_rename,
            )
            with pytest.raises(OSError):
                transaction_authority.commit_source_extension_candidate_transaction(
                    root,
                    custody=custody,
                    seal_sha256=seal_hash,
                    report_sha256=report_hash,
                )
        fail_source_extension_candidate_transaction(
            root, custody=custody, error="interrupted"
        )
        assert (
            json.loads((root / "candidate-transaction.json").read_text())["state"]
            == "prepared"
        )
        assert recover_and_prune_source_extension_candidate_transactions(
            custody=custody
        ) == (root,)
    finally:
        _release_file_lock(handle)
