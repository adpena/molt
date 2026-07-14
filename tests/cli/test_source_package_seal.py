from __future__ import annotations

import os
from pathlib import Path
import shutil

import pytest

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
    commit = prepare_source_package_seal_commit(
        transaction_root, seal, destination
    )
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
