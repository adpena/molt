from __future__ import annotations

import errno
import hashlib
import os
import shutil
import stat
import time
from pathlib import Path
import zipfile

import pytest

from molt.cli import atomic_io
from molt.cli import backend_cache
from molt.cli.backend_artifact_contract import (
    BackendArtifactContract,
    BackendArtifactKind,
)
from molt import artifact_publication
from molt import file_publication
from molt.cli.runtime_wasm_generation import publish_runtime_wasm_generation
from molt.wasm_artifact import (
    _build_wasm_sections,
    _write_wasm_string,
    transform_wasm_publication_file,
)
from tests.process_guard_common import current_thread_only, install_module_view
from tests.runtime_build_identity_helper import runtime_build_identity


def test_owned_path_has_one_spelling_before_and_after_allocation(tmp_path):
    path = tmp_path / "owned-generation"
    spellings = [path]
    if os.name == "nt":
        spellings.append(Path("\\\\?\\" + str(path)))
    before = [file_publication.resolve_owned_path(value) for value in spellings]
    path.mkdir()
    after = [file_publication.resolve_owned_path(value) for value in spellings]
    assert {str(value) for value in before + after} == {str(path.resolve())}


@pytest.mark.parametrize("existing", [False, True])
def test_verified_copy_checks_staged_bytes_before_publication(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, existing: bool
) -> None:
    source = tmp_path / "source"
    source.write_bytes(b"expected")
    destination = tmp_path / ("destination-" * 20)
    if existing:
        destination.write_bytes(b"previous")
    expected = hashlib.sha256(source.read_bytes()).hexdigest()
    copyfile = shutil.copyfile

    def changed_copy(src: Path, staged: Path) -> None:
        copyfile(src, staged)
        staged.write_bytes(b"changed during copy")

    # Artifact publication stages the verified copy.
    install_module_view(
        monkeypatch, "shutil", shutil, artifact_publication, copyfile=changed_copy
    )
    with pytest.raises(ValueError, match="source changed while staging verified copy"):
        atomic_io._atomic_copy_file(source, destination, expected_sha256=expected)
    assert not list(tmp_path.glob(".*.tmp"))
    if existing:
        assert destination.read_bytes() == b"previous"
    else:
        assert not destination.exists()
    monkeypatch.setattr(artifact_publication.shutil, "copyfile", copyfile)
    atomic_io._atomic_copy_file(source, destination, expected_sha256=expected)
    assert destination.read_bytes() == b"expected"


@pytest.mark.parametrize("name", ["x" * 234, "\u00e9" * 117])
@pytest.mark.parametrize("operation", ["bytes", "copy", "link", "fallback", "zip"])
def test_atomic_publication_accepts_long_destination_components(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, name: str, operation: str
) -> None:
    source = tmp_path / "source"
    source.write_bytes(b"payload")
    destination = tmp_path / name
    if operation == "bytes":
        atomic_io._atomic_write_bytes(destination, b"payload")
    elif operation == "copy":
        atomic_io._atomic_copy_file(source, destination)
    elif operation in {"link", "fallback"}:
        if operation == "fallback":

            def no_links(*_args, **_kwargs):
                raise OSError(errno.EXDEV, "fixture has no hard links")

            install_module_view(monkeypatch, "os", os, atomic_io, link=no_links)
        atomic_io._atomic_link_or_copy_file(source, destination)
    else:
        with atomic_io._atomic_zip_file(destination) as archive:
            archive.writestr("member", b"payload")
    if operation == "zip":
        with zipfile.ZipFile(destination) as archive:
            assert archive.read("member") == b"payload"
    else:
        assert destination.read_bytes() == b"payload"
    assert not list(tmp_path.glob(".molt-*.tmp"))


def test_nested_backend_cache_publication_uses_bounded_stages(tmp_path: Path) -> None:
    # Native cache keys carry three complete digests. The outer cache stage and
    # inner verified copy must not each append another nonce to that basename.
    digest = hashlib.sha256(b"cache-key").hexdigest()
    destination = tmp_path / f"{digest}.artifact-{digest}.stdlib-{digest}.a"
    source = tmp_path / "source.rs"
    source.write_bytes(b"fn alpha() {}\n")
    warnings: list[str] = []
    identity = backend_cache._publish_immutable_backend_cache_artifact(
        source,
        destination,
        artifact_contract=BackendArtifactContract(BackendArtifactKind.RUST),
        warnings=warnings,
    )
    assert identity.path == destination
    assert identity.sha256 == hashlib.sha256(source.read_bytes()).hexdigest()
    assert not source.samefile(destination)
    assert not warnings
    assert not list(tmp_path.glob(".molt-*.tmp"))


def test_every_atomic_publication_has_one_file_fsync_per_staged_file(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    calls: list[int] = []
    # File publication issues every publication barrier.
    install_module_view(
        monkeypatch, "os", os, file_publication, fsync=lambda fd: calls.append(fd)
    )
    barriers_per_commit = 2 if atomic_io.os.name == "posix" else 1

    atomic_io._atomic_write_text(tmp_path / "text", "value")
    assert len(calls) == barriers_per_commit
    calls.clear()
    atomic_io._atomic_write_bytes(tmp_path / "bytes", b"value")
    assert len(calls) == barriers_per_commit
    calls.clear()
    source = tmp_path / "source"
    source.write_bytes(b"source")
    atomic_io._atomic_copy_file(source, tmp_path / "copy")
    assert len(calls) == barriers_per_commit
    calls.clear()
    with atomic_io._atomic_zip_file(tmp_path / "archive.zip") as archive:
        archive.writestr("member", b"value")
    assert len(calls) == barriers_per_commit

    calls.clear()
    shared = tmp_path / "molt_runtime.wasm"
    reloc = tmp_path / "molt_runtime_reloc.wasm"
    shared.write_bytes(b"shared")
    reloc.write_bytes(b"reloc")
    publish_runtime_wasm_generation(
        shared,
        reloc,
        shared_identity=runtime_build_identity("shared", "atomic-publication"),
        reloc_identity=runtime_build_identity("reloc", "atomic-publication"),
    )
    # Immutable shared+reloc members, the atomic pair pointer and the store's
    # usage ledger. Internal publication deliberately creates no fixed-name
    # compatibility projections.
    assert len(calls) == 4 * barriers_per_commit

    calls.clear()
    wasm = tmp_path / "publication.wasm"
    wasm.write_bytes(
        _build_wasm_sections([(0, _write_wasm_string(".debug_info") + b"debug")])
    )
    transform_wasm_publication_file(
        wasm, rename_map={}, final_artifact=True, preserve_debug=False
    )
    assert len(calls) == barriers_per_commit


@pytest.mark.parametrize("changed", [False, True])
def test_wasm_transform_cleanup_requires_successful_stage_creation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, changed: bool
) -> None:
    wasm = tmp_path / "publication.wasm"
    original = _build_wasm_sections(
        [(0, _write_wasm_string(".debug_info") + b"debug")] if changed else []
    )
    wasm.write_bytes(original)
    stage = file_publication.staged_file_path(wasm, purpose="wasm-transform")
    stage.write_bytes(b"another owner's stage")
    before = stage.stat()
    monkeypatch.setattr(file_publication, "staged_file_path", lambda *a, **k: stage)

    if changed:
        with pytest.raises(FileExistsError):
            transform_wasm_publication_file(
                wasm, rename_map={}, final_artifact=True, preserve_debug=False
            )
    else:
        metrics = transform_wasm_publication_file(
            wasm, rename_map={}, final_artifact=True, preserve_debug=False
        )
        assert metrics.changed is False

    assert stage.read_bytes() == b"another owner's stage"
    assert os.path.samestat(before, stage.stat())
    assert wasm.read_bytes() == original


def test_windows_namespace_commit_requests_replace_and_write_through(
    tmp_path: Path,
) -> None:
    calls: list[tuple[str, str, int]] = []
    staged = tmp_path / "stage"
    destination = tmp_path / "destination"

    def fake_move_file_ex(src: str, dst: str, flags: int) -> int:
        calls.append((src, dst, flags))
        return 1

    file_publication.move_file_ex_write_through(
        staged,
        destination,
        move_file_ex=fake_move_file_ex,
    )

    assert calls == [
        (
            str(staged),
            str(destination),
            file_publication.MOVEFILE_REPLACE_EXISTING
            | file_publication.MOVEFILE_WRITE_THROUGH,
        )
    ]


@pytest.mark.parametrize("operation", ["bytes", "text", "json", "copy"])
def test_reconstructible_publication_is_complete_without_durability_barriers(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, operation: str
) -> None:
    def unexpected_flush(_fd: int) -> None:
        pytest.fail("disposable cache crossed a durability barrier")

    install_module_view(monkeypatch, "os", os, file_publication, fsync=unexpected_flush)
    destination = tmp_path / "cache.json"
    destination.write_bytes(b"previous")
    policy = file_publication.PublicationDurability.RECONSTRUCTIBLE
    if operation == "bytes":
        file_publication.atomic_write_bytes(destination, b"complete", durability=policy)
    elif operation == "text":
        atomic_io._atomic_write_text(destination, "complete", durability=policy)
    elif operation == "json":
        atomic_io._atomic_write_json(destination, {"complete": True}, durability=policy)
    else:
        source = tmp_path / "source"
        source.write_bytes(b"complete")
        atomic_io._atomic_copy_file(source, destination, durability=policy)
    assert destination.read_bytes() == (
        b'{\n  "complete": true\n}\n' if operation == "json" else b"complete"
    )
    assert not list(tmp_path.glob(".molt-*.tmp"))


def test_windows_reconstructible_replace_uses_atomic_replace_without_write_through(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    staged, destination = tmp_path / "stage", tmp_path / "destination"
    staged.write_bytes(b"complete")
    destination.write_bytes(b"previous")
    attempts = 0

    def replace(source: Path, target: Path) -> None:
        nonlocal attempts
        attempts += 1
        if attempts == 1:
            error = PermissionError("sharing violation")
            error.winerror = 32  # type: ignore[attr-defined]
            raise error
        os.replace(source, target)

    install_module_view(
        monkeypatch, "os", os, file_publication, name="nt", replace=replace
    )
    monkeypatch.setattr(
        file_publication,
        "windows_replace_write_through",
        lambda *_args: pytest.fail("cache requested WRITE_THROUGH"),
    )
    install_module_view(
        monkeypatch, "time", time, file_publication, sleep=lambda _: None
    )
    file_publication.atomic_replace(
        staged,
        destination,
        durability=file_publication.PublicationDurability.RECONSTRUCTIBLE,
    )
    assert attempts == 2
    assert destination.read_bytes() == b"complete"
    assert not staged.exists()


@pytest.mark.parametrize("operation", ["bytes", "copy"])
def test_reconstructible_replace_failure_preserves_old_leaf_and_cleans_stage(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, operation: str
) -> None:
    destination = tmp_path / "destination"
    destination.write_bytes(b"previous")
    destination.chmod(stat.S_IREAD)
    old_mode = destination.stat().st_mode
    primary = OSError("namespace commit refused")

    def fail_replace(_source: Path, _destination: Path) -> None:
        raise primary

    install_module_view(monkeypatch, "os", os, file_publication, replace=fail_replace)
    policy = file_publication.PublicationDurability.RECONSTRUCTIBLE
    with pytest.raises(OSError) as caught:
        if operation == "bytes":
            file_publication.atomic_write_bytes(
                destination, b"complete", durability=policy
            )
        else:
            source = tmp_path / "source"
            source.write_bytes(b"complete")
            atomic_io._atomic_copy_file(source, destination, durability=policy)
    assert caught.value is primary
    assert destination.read_bytes() == b"previous"
    assert destination.stat().st_mode == old_mode
    assert not list(tmp_path.glob(".molt-*.tmp"))


def test_reconstructible_readonly_stage_needs_no_write_permission(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    staged, destination = tmp_path / "stage", tmp_path / "destination"
    staged.write_bytes(b"complete")
    staged.chmod(stat.S_IREAD)
    monkeypatch.setattr(
        Path,
        "chmod",
        current_thread_only(
            lambda *_args: pytest.fail("cache stage requested write access"),
            Path.chmod,
        ),
    )
    file_publication.atomic_replace(
        staged,
        destination,
        durability=file_publication.PublicationDurability.RECONSTRUCTIBLE,
    )
    assert destination.read_bytes() == b"complete"
    assert not destination.stat().st_mode & stat.S_IWRITE


@pytest.mark.parametrize("invalid", ["unknown", "exclusive", "callback"])
def test_invalid_publication_policy_fails_before_filesystem_effects(
    tmp_path: Path, invalid: str
) -> None:
    parent = tmp_path / "never-created"
    options = {"durability": file_publication.PublicationDurability.RECONSTRUCTIBLE}
    if invalid == "unknown":
        options["durability"] = "reconstructible"
    elif invalid == "exclusive":
        options["exclusive"] = True
    else:
        options["replace"] = lambda *_args: None
    with pytest.raises(ValueError):
        file_publication.atomic_write_bytes(parent / "cache", b"value", **options)
    assert not parent.exists()


def test_windows_write_through_replace_retries_only_sharing_violations(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    calls = 0

    def transient(_src: Path, _dst: Path) -> None:
        nonlocal calls
        calls += 1
        if calls == 1:
            error = PermissionError("sharing violation")
            error.winerror = 32  # type: ignore[attr-defined]
            raise error

    install_module_view(
        monkeypatch, "time", time, file_publication, sleep=lambda _seconds: None
    )

    file_publication.windows_replace_write_through(
        tmp_path / "stage",
        tmp_path / "destination",
        replace_once=transient,
    )

    assert calls == 2


def test_readonly_copy_flushes_payload_and_preserves_final_mode(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "readonly-source"
    destination = tmp_path / "destination"
    source.write_bytes(b"immutable")
    source.chmod(stat.S_IREAD)
    original = os.fsync
    flushed_files: list[tuple[int, int]] = []

    def observe(descriptor: int) -> None:
        metadata = os.fstat(descriptor)
        original(descriptor)
        if stat.S_ISREG(metadata.st_mode):
            flushed_files.append((metadata.st_size, metadata.st_mode))

    install_module_view(monkeypatch, "os", os, file_publication, fsync=observe)
    atomic_io._atomic_copy_file(source, destination)

    assert len(flushed_files) == 1
    assert flushed_files[0][0] == len(b"immutable")
    assert flushed_files[0][1] & stat.S_IWRITE
    assert not destination.stat().st_mode & stat.S_IWRITE
    assert destination.read_bytes() == b"immutable"

    replacement = tmp_path / "readonly-replacement"
    replacement.write_bytes(b"replacement")
    replacement.chmod(stat.S_IREAD)
    atomic_io._atomic_copy_file(replacement, destination)
    assert len(flushed_files) == 2
    assert flushed_files[1][0] == len(b"replacement")
    assert flushed_files[1][1] & stat.S_IWRITE
    assert destination.read_bytes() == b"replacement"
    assert not destination.stat().st_mode & stat.S_IWRITE


def test_observed_copy_rejects_source_replacement_before_copy(
    tmp_path: Path,
) -> None:
    from molt.toolchain_identity import stable_regular_file_identity

    source = tmp_path / "source"
    destination = tmp_path / "destination"
    source.write_bytes(b"source")
    destination.write_bytes(b"previous")
    observed = stable_regular_file_identity(source, label="copy fixture")
    replacement = tmp_path / "replacement"
    replacement.write_bytes(b"source")
    replacement.replace(source)
    with pytest.raises(ValueError, match="changed since identity capture"):
        atomic_io._atomic_copy_file(source, destination, observed=observed)
    assert destination.read_bytes() == b"previous"
    assert not list(tmp_path.glob(".molt-*.tmp"))


def test_observed_copy_rejects_same_size_content_with_matching_metadata(
    tmp_path: Path,
) -> None:
    from dataclasses import replace
    from molt.toolchain_identity import stable_regular_file_identity

    source = tmp_path / "source"
    destination = tmp_path / "destination"
    source.write_bytes(b"source")
    destination.write_bytes(b"previous")
    observed = stable_regular_file_identity(source, label="copy fixture")
    before = source.stat()
    source.write_bytes(b"mutate")
    os.utime(source, ns=(before.st_atime_ns, before.st_mtime_ns))
    current = stable_regular_file_identity(source, label="current metadata fixture")
    observed = replace(current, sha256=observed.sha256)
    with pytest.raises(ValueError, match="content changed"):
        atomic_io._atomic_copy_file(source, destination, observed=observed)
    assert destination.read_bytes() == b"previous"
    assert not list(tmp_path.glob(".molt-*.tmp"))


@pytest.mark.parametrize("exclusive", [False, True])
@pytest.mark.parametrize("primary_type", [OSError, KeyboardInterrupt])
def test_atomic_write_cleanup_failure_preserves_primary_error(
    tmp_path, monkeypatch, exclusive, primary_type
):
    destination = tmp_path / "destination"
    destination.write_bytes(b"previous")
    if exclusive:
        destination.unlink()
    stage = tmp_path / ".molt-fixture.tmp"
    primary = primary_type("primary publication failed")
    cleanup = PermissionError("stage cleanup denied")
    original_unlink = Path.unlink
    monkeypatch.setattr(file_publication, "staged_file_path", lambda destination: stage)

    def fail_publish(*args):
        raise primary

    def fail_cleanup(path, *args, **kwargs):
        if path == stage:
            raise cleanup
        return original_unlink(path, *args, **kwargs)

    monkeypatch.setattr(file_publication, "durable_replace", fail_publish)
    monkeypatch.setattr(file_publication, "durable_publish_exclusive", fail_publish)
    monkeypatch.setattr(Path, "unlink", fail_cleanup)
    with pytest.raises(primary_type) as caught:
        file_publication.atomic_write_bytes(
            destination, b"replacement", exclusive=exclusive
        )
    assert caught.value is primary
    assert any(
        "stage cleanup denied" in note and str(stage) in note
        for note in primary.__notes__
    )
    assert stage.read_bytes() == b"replacement"
    assert (
        not destination.exists()
        if exclusive
        else destination.read_bytes() == b"previous"
    )


def test_atomic_write_cleanup_failure_without_primary_remains_visible(
    tmp_path, monkeypatch
):
    destination = tmp_path / "destination"
    stage = tmp_path / ".molt-fixture.tmp"
    cleanup = PermissionError("stage cleanup denied")
    original_unlink = Path.unlink
    monkeypatch.setattr(file_publication, "staged_file_path", lambda destination: stage)

    def publish(staged, final):
        final.write_bytes(staged.read_bytes())

    def fail_cleanup(path, *args, **kwargs):
        if path == stage:
            raise cleanup
        return original_unlink(path, *args, **kwargs)

    monkeypatch.setattr(Path, "unlink", fail_cleanup)
    with pytest.raises(PermissionError) as caught:
        file_publication.atomic_write_bytes(
            destination, b"replacement", replace=publish
        )
    assert caught.value is cleanup
    assert destination.read_bytes() == stage.read_bytes() == b"replacement"


@pytest.mark.parametrize("publication_failed", [False, True])
def test_atomic_write_missing_stage_cleanup_is_ignored(
    tmp_path, monkeypatch, publication_failed
):
    destination = tmp_path / "destination"
    stage = tmp_path / ".molt-fixture.tmp"
    primary = OSError("primary publication failed")
    monkeypatch.setattr(file_publication, "staged_file_path", lambda destination: stage)

    def publish(staged, final):
        if publication_failed:
            staged.unlink()
            raise primary
        staged.replace(final)

    if publication_failed:
        with pytest.raises(OSError) as caught:
            file_publication.atomic_write_bytes(
                destination, b"replacement", replace=publish
            )
        assert caught.value is primary
        assert not getattr(primary, "__notes__", ())
        assert not destination.exists()
    else:
        file_publication.atomic_write_bytes(
            destination, b"replacement", replace=publish
        )
        assert destination.read_bytes() == b"replacement"
    assert not stage.exists()


def test_signed_staged_copy_is_a_pure_function_of_bytes_and_destination(
    tmp_path: Path,
) -> None:
    """Two publications of one binary to one name produce identical signed bytes.

    Every stage gets a unique random name. On macOS an ad-hoc signature takes
    its identifier from the file name unless the caller fixes it, so signing
    the stage used to embed the random name and change the published bytes on
    every build. Other hosts do not sign, which this oracle also covers.
    """
    import sys

    source = tmp_path / "source-binary"
    source.write_bytes(Path(sys.executable).resolve().read_bytes())
    source.chmod(0o755)
    produced: list[bytes] = []
    for generation in ("first", "second"):
        destination = tmp_path / generation / "molt-app"
        destination.parent.mkdir()
        with atomic_io._staged_copy_file(source, destination, codesign=True) as stage:
            assert stage.name != destination.name
            produced.append(stage.read_bytes())
    assert produced[0] == produced[1]
