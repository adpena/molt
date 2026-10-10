"""A transferred native toolchain is admitted by the importing checkout's build.

The oracle is the build's own admission: after an import, the runtime
candidate lookup must select the transferred generation under the importing
checkout's identity, and ``_ensure_backend_binary`` must admit the transferred
backend without running Cargo.
"""

from __future__ import annotations

from pathlib import Path
import shutil
import subprocess

import pytest

from molt.cli import backend_binary
from molt.cli import native_toolchain_transfer as transfer
from molt.cli import runtime_native_build
from tests.cli.native_toolchain_test_support import (
    BACKEND_FEATURES,
    BACKEND_PROFILE,
    RUNTIME_PROFILE,
    checkout,
    runtime_identity,
)


def _transport(members, destination: Path) -> dict[str, Path]:
    destination.mkdir()
    files = {}
    for member in members:
        copy = destination / member.role
        shutil.copyfile(member.path, copy)
        copy.chmod(0o755 if member.executable else 0o644)
        files[member.role] = copy
    return files


def _exported(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> dict[str, Path]:
    producer = checkout(tmp_path, monkeypatch, "producer")
    producer.build(monkeypatch)
    return _transport(
        transfer.export_native_toolchain(producer.selection), tmp_path / "transport"
    )


def _forbid_cargo(monkeypatch: pytest.MonkeyPatch) -> None:
    def cargo(*_args, **_kwargs):
        raise AssertionError("an imported backend must not be rebuilt")

    monkeypatch.setattr(backend_binary, "_run_resolved_cargo_plan", cargo)
    monkeypatch.setattr(
        backend_binary,
        "_run_subprocess_captured_to_tempfiles",
        lambda cmd, **_kwargs: subprocess.CompletedProcess(cmd, 0, b"", b""),
    )


def test_export_names_the_admitted_runtime_generation_and_backend(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    producer = checkout(tmp_path, monkeypatch, "producer")
    producer.build(monkeypatch)

    members = transfer.export_native_toolchain(producer.selection)

    assert [member.role for member in members] == [
        transfer.RUNTIME_SELECTION_ROLE,
        transfer.RUNTIME_ARCHIVE_ROLE,
        transfer.NATIVE_LINK_MANIFEST_ROLE,
        transfer.BACKEND_EXECUTABLE_ROLE,
        transfer.BACKEND_RECEIPT_ROLE,
    ]
    assert [member.executable for member in members] == [
        False,
        False,
        False,
        True,
        False,
    ]
    assert members[3].path == producer.selection.backend.binary


def test_imported_toolchain_is_admitted_by_the_next_build_without_cargo(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    files = _exported(tmp_path, monkeypatch)
    consumer = checkout(tmp_path, monkeypatch, "consumer")
    consumer.activate(monkeypatch)

    transfer.import_native_toolchain(consumer.selection, files)

    selection = consumer.selection
    candidates = runtime_native_build._native_runtime_generation_candidates(
        selection.runtime_lib,
        project_root=consumer.root,
        cargo_profile=RUNTIME_PROFILE,
        target_triple=None,
    )
    assert [generation.build_identity for generation in candidates] == [
        runtime_identity()
    ]
    assert candidates[0].runtime_lib.is_relative_to(consumer.root / "target")
    _forbid_cargo(monkeypatch)
    assert backend_binary._ensure_backend_binary(
        selection.backend.binary,
        cargo_timeout=1.0,
        json_output=True,
        cargo_profile=BACKEND_PROFILE,
        project_root=consumer.root,
        backend_features=BACKEND_FEATURES,
    )
    assert (
        selection.backend.binary.read_bytes()
        == files[transfer.BACKEND_EXECUTABLE_ROLE].read_bytes()
    )


def test_import_refuses_a_runtime_built_from_other_inputs(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    files = _exported(tmp_path, monkeypatch)
    consumer = checkout(tmp_path, monkeypatch, "consumer", runtime_seed="changed")
    consumer.activate(monkeypatch)

    with pytest.raises(transfer.NativeToolchainTransferError, match="other inputs"):
        transfer.import_native_toolchain(consumer.selection, files)

    assert not runtime_native_build._native_runtime_generation_candidates(
        consumer.selection.runtime_lib,
        project_root=consumer.root,
        cargo_profile=RUNTIME_PROFILE,
        target_triple=None,
    )
    assert not (consumer.root / "target" / BACKEND_PROFILE).exists()


def test_import_refuses_a_backend_built_from_other_sources(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    files = _exported(tmp_path, monkeypatch)
    consumer = checkout(tmp_path, monkeypatch, "consumer", backend_seed="changed")
    consumer.activate(monkeypatch)

    with pytest.raises(transfer.NativeToolchainTransferError, match="other inputs"):
        transfer.import_native_toolchain(consumer.selection, files)

    assert not (consumer.root / "target" / BACKEND_PROFILE).exists()


def test_import_refuses_a_backend_that_differs_from_its_receipt(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    files = _exported(tmp_path, monkeypatch)
    executable = files[transfer.BACKEND_EXECUTABLE_ROLE]
    executable.write_bytes(b"#!/bin/sh\nexit 0\n# substituted\n")
    consumer = checkout(tmp_path, monkeypatch, "consumer")
    consumer.activate(monkeypatch)

    with pytest.raises(transfer.NativeToolchainTransferError, match="receipt"):
        transfer.import_native_toolchain(consumer.selection, files)

    canonical = consumer.root / "target" / BACKEND_PROFILE
    assert not canonical.exists() or not any(canonical.iterdir())


@pytest.mark.parametrize("change", ["missing", "unknown"])
def test_import_requires_the_exact_file_set(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, change: str
) -> None:
    files = _exported(tmp_path, monkeypatch)
    if change == "missing":
        del files[transfer.BACKEND_RECEIPT_ROLE]
    else:
        files["extra"] = files[transfer.BACKEND_RECEIPT_ROLE]
    consumer = checkout(tmp_path, monkeypatch, "consumer")
    consumer.activate(monkeypatch)

    with pytest.raises(transfer.NativeToolchainTransferError, match=change):
        transfer.import_native_toolchain(consumer.selection, files)
    assert not (consumer.root / "target").exists()


def test_export_requires_an_admitted_runtime(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    producer = checkout(tmp_path, monkeypatch, "producer")
    producer.activate(monkeypatch)

    with pytest.raises(
        transfer.NativeToolchainTransferError, match="no admitted generation"
    ):
        transfer.export_native_toolchain(producer.selection)


def test_export_names_a_generation_built_from_other_inputs(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    producer = checkout(tmp_path, monkeypatch, "producer")
    producer.build(monkeypatch)
    producer.runtime_seed = "edited-after-build"
    producer.activate(monkeypatch)

    with pytest.raises(
        transfer.NativeToolchainTransferError,
        match="generation built from other inputs",
    ):
        transfer.export_native_toolchain(producer.selection)


def test_export_requires_a_backend_admitted_for_this_checkout(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    producer = checkout(tmp_path, monkeypatch, "producer")
    producer.build(monkeypatch)
    producer.backend_seed = "edited-after-build"
    producer.activate(monkeypatch)

    with pytest.raises(transfer.NativeToolchainTransferError, match="backend"):
        transfer.export_native_toolchain(producer.selection)
