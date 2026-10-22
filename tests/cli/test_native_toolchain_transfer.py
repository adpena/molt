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
import time

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


def _published_files(consumer) -> dict[Path, tuple[int, int, bytes]]:
    """Every file of every published entry: identity, time and bytes.

    The ledgers and locks are store bookkeeping, not entry content.
    """
    files = {}
    for store in (consumer.runtime_store, consumer.backend_store):
        for path in sorted(store.root.rglob("*")):
            if path.is_file() and not path.name.startswith(".product-store"):
                if path.parent == store.root / ".locks":
                    continue
                stat = path.stat()
                files[path] = (stat.st_ino, stat.st_mtime_ns, path.read_bytes())
    return files


def test_a_repeated_import_never_rewrites_a_published_variant(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Readers may hold what a variant selects, so a second import keeps it."""
    files = _exported(tmp_path, monkeypatch)
    consumer = checkout(tmp_path, monkeypatch, "consumer")
    consumer.activate(monkeypatch)
    transfer.import_native_toolchain(consumer.selection, files)
    published = _published_files(consumer)
    assert any(path.suffix == ".fingerprint" for path in published)
    assert any(path.name.endswith(".generation.json") for path in published)

    transfer.import_native_toolchain(consumer.selection, files)

    assert _published_files(consumer) == published


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
    assert not consumer.stored_backend_variants()


def test_import_refuses_a_backend_built_from_other_sources(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    files = _exported(tmp_path, monkeypatch)
    consumer = checkout(tmp_path, monkeypatch, "consumer", backend_seed="changed")
    consumer.activate(monkeypatch)

    with pytest.raises(transfer.NativeToolchainTransferError, match="other inputs"):
        transfer.import_native_toolchain(consumer.selection, files)

    assert not consumer.stored_backend_variants()


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

    assert not consumer.stored_backend_variants()


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
    assert not consumer.backend_store.root.exists()


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


def test_export_renews_both_entries_before_it_reads_them(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A read renews its entry first, so no eviction can race the export.

    Both entries were last used 30 s inside the idle window, where the next
    publication's eviction could remove them during the export's read; the
    export renews them to now before it reads.
    """
    import json

    from molt.cli import product_store

    producer = checkout(tmp_path, monkeypatch, "producer")
    producer.build(monkeypatch)
    stores = (producer.runtime_store, producer.backend_store)
    aged = time.time() - product_store.PRODUCT_STORE_IDLE_SECONDS + 30
    entries = {}
    for store in stores:
        (entry,) = store.root.glob("[0-9a-f]" * 64)
        identity = entry.name
        store.record_use(identity)
        ledger_path = store.root / ".product-store.json"
        ledger = json.loads(ledger_path.read_text(encoding="utf-8"))
        ledger["last_used"][identity] = aged
        ledger_path.write_text(json.dumps(ledger), encoding="utf-8")
        entries[store] = identity

    transfer.export_native_toolchain(producer.selection)

    for store, identity in entries.items():
        ledger = json.loads((store.root / ".product-store.json").read_bytes())
        assert ledger["last_used"][identity] > aged + 60
        assert store.entry(identity).is_dir()
