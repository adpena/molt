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
from molt.cli.rust_cpu_policy import RustCpuPolicy
from tests.cli.backend_generation_test_support import selected_backend
from tests.cli.native_toolchain_test_support import (
    BACKEND_FEATURES,
    BACKEND_PROFILE,
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


def test_current_selection_is_the_baseline_cpu(tmp_path, monkeypatch):
    monkeypatch.delenv("MOLT_PERF_PROFILE", raising=False)

    selection = transfer.NativeToolchainSelection.current(tmp_path)

    assert selection.backend.rust_cpu is RustCpuPolicy.BASELINE


@pytest.mark.parametrize(
    ("value", "message"),
    [("native-arch", "never transfer"), ("native", "MOLT_PERF_PROFILE must be")],
)
def test_current_selection_refuses_a_host_cpu_build(
    tmp_path, monkeypatch, value, message
):
    # -C target-cpu=native names no CPU, so its identity cannot tell two hosts
    # apart; a transported native-arch product could fault on another CPU.
    monkeypatch.setenv("MOLT_PERF_PROFILE", value)

    with pytest.raises(transfer.NativeToolchainTransferError, match=message):
        transfer.NativeToolchainSelection.current(tmp_path)


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
    assert members[3].path == selected_backend(
        producer.root, producer.selection.backend.binary, BACKEND_PROFILE
    )


def test_imported_toolchain_is_admitted_by_the_next_build_without_cargo(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    files = _exported(tmp_path, monkeypatch)
    consumer = checkout(tmp_path, monkeypatch, "consumer")
    consumer.activate(monkeypatch)

    transfer.import_native_toolchain(consumer.selection, files)

    selection = consumer.selection
    candidates = consumer.stored_runtimes()
    assert [generation.build_identity for generation in candidates] == [
        runtime_identity()
    ]
    assert candidates[0].runtime_lib.is_relative_to(consumer.runtime_store.root)
    _forbid_cargo(monkeypatch)
    result = backend_binary._ensure_backend_binary(
        selection.backend.binary,
        cargo_timeout=1.0,
        json_output=True,
        cargo_profile=BACKEND_PROFILE,
        project_root=consumer.root,
        backend_features=BACKEND_FEATURES,
        rust_cpu=RustCpuPolicy.BASELINE,
    )
    assert result, result.message
    assert result.binary is not None
    assert (
        result.binary.read_bytes()
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

    # Neither the consumer's identity nor the exported one gained an entry.
    assert not consumer.stored_runtimes()
    assert not consumer.stored_runtimes("runtime")
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
    assert not consumer.runtime_store.root.exists()


def test_export_requires_an_admitted_runtime(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    producer = checkout(tmp_path, monkeypatch, "producer")
    producer.activate(monkeypatch)

    with pytest.raises(
        transfer.NativeToolchainTransferError,
        match="is stored for this checkout's inputs",
    ):
        transfer.export_native_toolchain(producer.selection)


def test_export_refuses_a_runtime_stored_only_for_other_inputs(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    producer = checkout(tmp_path, monkeypatch, "producer")
    producer.build(monkeypatch)
    producer.runtime_seed = "edited-after-build"
    producer.activate(monkeypatch)

    with pytest.raises(
        transfer.NativeToolchainTransferError,
        match="is stored for this checkout's inputs",
    ):
        transfer.export_native_toolchain(producer.selection)
    # The generation of the earlier inputs stays in the store under its own key.
    assert producer.stored_runtimes("runtime")


def test_export_requires_a_backend_admitted_for_this_checkout(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    producer = checkout(tmp_path, monkeypatch, "producer")
    producer.build(monkeypatch)
    producer.backend_seed = "edited-after-build"
    producer.activate(monkeypatch)

    with pytest.raises(transfer.NativeToolchainTransferError, match="backend"):
        transfer.export_native_toolchain(producer.selection)
