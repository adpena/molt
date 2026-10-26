from __future__ import annotations

import ctypes
import errno
import hashlib
import os
import shutil
import stat
import sys
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
from tests.runtime_build_identity_helper import (
    runtime_build_identity,
    runtime_cargo_environment_inputs,
)


def test_clone_file_copies_into_a_missing_leaf_with_its_own_inode(tmp_path):
    source = tmp_path / "source.bin"
    payload = bytes(range(256)) * 4096
    source.write_bytes(payload)
    destination = tmp_path / "destination.bin"
    file_publication.clone_file(source, destination)
    assert destination.read_bytes() == payload
    assert destination.stat().st_ino != source.stat().st_ino
    # A clone shares blocks copy-on-write: rewriting the source cannot reach it.
    with source.open("r+b") as handle:
        handle.write(b"rewritten")
    assert destination.read_bytes() == payload
    with pytest.raises(FileExistsError):
        file_publication.clone_file(source, destination)
    assert destination.read_bytes() == payload


@pytest.mark.skipif(
    sys.platform != "darwin" and not sys.platform.startswith("linux"),
    reason="the clone system call is platform specific",
)
def test_clone_file_copies_when_the_volume_cannot_share_blocks(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "source.bin"
    source.write_bytes(b"payload")
    attempts: list[str] = []
    if sys.platform == "darwin":

        class Libc:
            def __init__(self, *_args, **_kwargs) -> None:
                self.fclonefileat = self

            def __call__(self, *_args) -> int:
                attempts.append("fclonefileat")
                ctypes.set_errno(errno.EXDEV)
                return -1

        install_module_view(monkeypatch, "ctypes", ctypes, file_publication, CDLL=Libc)
    else:
        import fcntl

        def ioctl(*_args) -> None:
            attempts.append("ficlone")
            raise OSError(errno.EOPNOTSUPP, "no reflink")

        monkeypatch.setattr(fcntl, "ioctl", ioctl)
    destination = tmp_path / "destination.bin"
    file_publication.clone_file(source, destination)
    assert attempts
    assert destination.read_bytes() == b"payload"
    with pytest.raises(FileExistsError):
        file_publication.clone_file(source, destination)


def test_clone_file_follows_source_symlinks_without_aliasing_bytes(tmp_path):
    target = tmp_path / "target.bin"
    target.write_bytes(b"source bytes")
    source = tmp_path / "source-link"
    try:
        source.symlink_to(target)
    except OSError as exc:
        pytest.skip(f"symlink creation unavailable: {exc}")
    destination = tmp_path / "result.bin"
    file_publication.clone_file(source, destination)
    assert not destination.is_symlink()
    assert destination.read_bytes() == b"source bytes"
    target.write_bytes(b"changed")
    assert destination.read_bytes() == b"source bytes"


def test_failed_kernel_copy_reclaims_only_its_own_partial_destination(
    tmp_path, monkeypatch
):
    source = tmp_path / "source"
    source.write_bytes(b"complete")
    destination = tmp_path / "destination"
    primary = OSError(errno.EIO, "copy interrupted")

    def fail_copy(_source, target, *, length):
        target.write(b"partial")
        raise primary

    # The portable path must enforce the same acquisition boundary as clones.
    install_module_view(monkeypatch, "sys", sys, file_publication, platform="win32")
    install_module_view(
        monkeypatch, "shutil", shutil, file_publication, copyfileobj=fail_copy
    )
    with pytest.raises(OSError) as caught:
        file_publication.clone_file(source, destination)
    assert caught.value is primary
    assert not destination.exists()
    destination.write_bytes(b"rival")
    with pytest.raises(FileExistsError):
        file_publication.clone_file(source, destination)
    assert destination.read_bytes() == b"rival"
    assert source.read_bytes() == b"complete"


@pytest.mark.parametrize("indirect", [False, True], ids=["directory", "directory-link"])
def test_clone_file_refuses_nonregular_source_before_creating_destination(
    tmp_path, monkeypatch, indirect
):
    source = tmp_path / "source"
    source.mkdir()
    (source / "child").write_bytes(b"must not clone a directory")
    if indirect:
        link = tmp_path / "source-link"
        try:
            link.symlink_to(source, target_is_directory=True)
        except OSError as exc:
            pytest.skip(f"symlink creation unavailable: {exc}")
        source = link
    destination = tmp_path / "destination"
    opened = []

    def observe_open(path, flags):
        descriptor = os.open(path, flags)
        opened.append(descriptor)
        return descriptor

    install_module_view(monkeypatch, "os", os, file_publication, open=observe_open)
    with pytest.raises(OSError):
        file_publication.clone_file(source, destination)
    assert not destination.exists()
    for descriptor in opened:
        with pytest.raises(OSError) as caught:
            os.fstat(descriptor)
        assert caught.value.errno == errno.EBADF


@pytest.mark.skipif(os.name != "posix", reason="POSIX permits replacing an open file")
@pytest.mark.parametrize("fail", [False, True], ids=["copy-success", "copy-failure"])
@pytest.mark.parametrize("replacement", ["file", "symlink"])
def test_copy_preserves_replacement_of_its_acquired_destination(
    tmp_path, monkeypatch, fail, replacement
):
    source = tmp_path / "source"
    source.write_bytes(b"complete")
    destination = tmp_path / "destination"
    displaced = tmp_path / "displaced-owned-file"
    rival = tmp_path / "rival"
    rival.write_bytes(b"rival bytes")
    primary = OSError(errno.EIO, "copy interrupted")

    def replace_then_copy(reader, writer, *, length):
        destination.rename(displaced)
        if replacement == "symlink":
            destination.symlink_to(rival)
        else:
            shutil.copyfile(rival, destination)
        writer.write(reader.read())
        if fail:
            raise primary

    install_module_view(monkeypatch, "sys", sys, file_publication, platform="win32")
    install_module_view(
        monkeypatch,
        "shutil",
        shutil,
        file_publication,
        copyfileobj=replace_then_copy,
    )
    with pytest.raises(OSError if fail else ValueError) as caught:
        file_publication.clone_file(source, destination)
    if fail:
        assert caught.value is primary
    assert destination.read_bytes() == b"rival bytes"
    assert destination.is_symlink() == (replacement == "symlink")
    assert rival.read_bytes() == b"rival bytes"
    assert displaced.read_bytes() == b"complete"


@pytest.mark.skipif(os.name != "posix", reason="POSIX permits replacing an open file")
def test_copy_reads_the_acquired_source_after_its_path_is_replaced(
    tmp_path, monkeypatch
):
    source = tmp_path / "source"
    source.write_bytes(b"acquired bytes")
    rival = tmp_path / "rival"
    rival.write_bytes(b"replacement bytes")
    destination = tmp_path / "destination"
    copyfileobj = shutil.copyfileobj

    def replace_then_copy(reader, writer, *, length):
        rival.replace(source)
        copyfileobj(reader, writer, length=length)

    install_module_view(monkeypatch, "sys", sys, file_publication, platform="win32")
    install_module_view(
        monkeypatch,
        "shutil",
        shutil,
        file_publication,
        copyfileobj=replace_then_copy,
    )
    file_publication.clone_file(source, destination)
    assert destination.read_bytes() == b"acquired bytes"
    assert source.read_bytes() == b"replacement bytes"


@pytest.mark.skipif(sys.platform != "darwin", reason="real Darwin descriptor clone")
def test_darwin_clone_uses_the_acquired_regular_source(tmp_path, monkeypatch):
    source = tmp_path / "source"
    source.write_bytes(b"acquired bytes")
    source.chmod(0o751)
    replacement = tmp_path / "replacement"
    replacement.mkdir()
    destination = tmp_path / "destination"
    libc = ctypes.CDLL(None, use_errno=True)
    native = libc.fclonefileat
    native.argtypes = (ctypes.c_int, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint32)
    native.restype = ctypes.c_int
    calls = []

    class Libc:
        def __init__(self, *_args, **_kwargs):
            self.fclonefileat = self

        def __call__(self, source_fd, parent_fd, leaf, flags):
            calls.append(os.fstat(source_fd).st_mode)
            source.rename(tmp_path / "old-source")
            source.symlink_to(replacement, target_is_directory=True)
            return native(source_fd, parent_fd, leaf, flags)

    install_module_view(monkeypatch, "ctypes", ctypes, file_publication, CDLL=Libc)
    file_publication.clone_file(source, destination)
    assert len(calls) == 1 and stat.S_ISREG(calls[0])
    assert destination.read_bytes() == b"acquired bytes"
    assert stat.S_IMODE(destination.stat().st_mode) == 0o751


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
    clone_file = artifact_publication.clone_file

    def changed_copy(src: Path, staged: Path) -> None:
        clone_file(src, staged)
        staged.write_bytes(b"changed during copy")

    # Artifact publication stages the verified copy with the platform copy.
    monkeypatch.setattr(artifact_publication, "clone_file", changed_copy)
    with pytest.raises(ValueError, match="source changed while staging verified copy"):
        atomic_io._atomic_copy_file(source, destination, expected_sha256=expected)
    assert not list(tmp_path.glob(".*.tmp"))
    if existing:
        assert destination.read_bytes() == b"previous"
    else:
        assert not destination.exists()
    monkeypatch.setattr(artifact_publication, "clone_file", clone_file)
    atomic_io._atomic_copy_file(source, destination, expected_sha256=expected)
    assert destination.read_bytes() == b"expected"
    # The clone has its own inode: rewriting the source cannot reach it.
    source.write_bytes(b"rewritten")
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
    ).identity
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
        cargo_environment=runtime_cargo_environment_inputs(),
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


def test_reconstructible_exclusive_publication_refuses_a_rival_without_barriers(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Exclusive create is a no-replace rename; its atomicity needs no flush."""

    flushes: list[int] = []
    install_module_view(monkeypatch, "os", os, file_publication, fsync=flushes.append)
    policy = file_publication.PublicationDurability.RECONSTRUCTIBLE
    destination = tmp_path / "record.json"
    file_publication.atomic_write_bytes(
        destination, b"first", exclusive=True, durability=policy
    )
    with pytest.raises(FileExistsError):
        file_publication.atomic_write_bytes(
            destination, b"rival", exclusive=True, durability=policy
        )
    assert destination.read_bytes() == b"first"
    assert flushes == []
    assert not list(tmp_path.glob(".molt-*.tmp"))
    # The durable spelling of the same publication still flushes the stage,
    # and on POSIX its directory.
    file_publication.atomic_write_bytes(tmp_path / "durable.json", b"x", exclusive=True)
    assert len(flushes) == (2 if os.name == "posix" else 1)


def test_windows_reconstructible_exclusive_rename_skips_write_through(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    renames: list[tuple[Path, Path]] = []

    def windows_rename(source: Path, target: Path) -> None:
        # MoveFileExW without REPLACE_EXISTING refuses a rival.
        if os.path.lexists(target):
            raise FileExistsError(target)
        renames.append((Path(source), Path(target)))
        os.rename(source, target)

    install_module_view(
        monkeypatch, "os", os, file_publication, name="nt", rename=windows_rename
    )
    monkeypatch.setattr(
        file_publication,
        "windows_replace_write_through",
        lambda *_args, **_kwargs: pytest.fail("reconstructible rename wrote through"),
    )
    policy = file_publication.PublicationDurability.RECONSTRUCTIBLE
    destination = tmp_path / "record.json"
    file_publication.atomic_write_bytes(
        destination, b"first", exclusive=True, durability=policy
    )
    with pytest.raises(FileExistsError):
        file_publication.atomic_write_bytes(
            destination, b"rival", exclusive=True, durability=policy
        )
    moved = tmp_path / "moved"
    file_publication.namespace_move_exclusive(destination, moved)
    assert moved.read_bytes() == b"first"
    assert [target for _source, target in renames] == [destination, moved]


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


@pytest.mark.parametrize("invalid", ["unknown", "callback"])
def test_invalid_publication_policy_fails_before_filesystem_effects(
    tmp_path: Path, invalid: str
) -> None:
    parent = tmp_path / "never-created"
    options = {"durability": file_publication.PublicationDurability.RECONSTRUCTIBLE}
    if invalid == "unknown":
        options["durability"] = "reconstructible"
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


@pytest.mark.parametrize("route", ["ordinary", "exclusive", "if-changed"])
@pytest.mark.parametrize("primary_type", [OSError, KeyboardInterrupt])
def test_atomic_write_cleanup_failure_preserves_primary_error(
    tmp_path, monkeypatch, route, primary_type
):
    exclusive = route == "exclusive"
    destination = tmp_path / "destination"
    destination.write_bytes(b"previous")
    if exclusive:
        destination.unlink()
    stage = tmp_path / ".molt-fixture.tmp"
    primary = primary_type("primary publication failed")
    cleanup = PermissionError("stage cleanup denied")
    original_unlink = Path.unlink
    monkeypatch.setattr(
        file_publication,
        "staged_file_path",
        lambda destination, *, purpose="write": stage,
    )

    def fail_publish(*args, **kwargs):
        raise primary

    def fail_cleanup(path, *args, **kwargs):
        if path == stage:
            raise cleanup
        return original_unlink(path, *args, **kwargs)

    monkeypatch.setattr(file_publication, "durable_replace", fail_publish)
    monkeypatch.setattr(file_publication, "_publish_exclusive", fail_publish)
    monkeypatch.setattr(Path, "unlink", fail_cleanup)
    with pytest.raises(primary_type) as caught:
        if route == "if-changed":
            atomic_io._atomic_write_text(
                destination, "replacement", skip_if_unchanged=True
            )
        else:
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
    monkeypatch.setattr(
        file_publication,
        "staged_file_path",
        lambda destination, *, purpose="write": stage,
    )

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
    monkeypatch.setattr(
        file_publication,
        "staged_file_path",
        lambda destination, *, purpose="write": stage,
    )

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


def test_stage_destination_key_is_routing_not_physical_custody(tmp_path):
    destination = tmp_path / "結果.json"
    stage = file_publication.staged_file_path(destination)
    key = file_publication.staging_destination_key(destination)
    assert (
        file_publication.staged_file_destination_key(
            stage, purpose="write", suffix=".tmp"
        )
        == key
    )
    assert file_publication.is_owned_staged_file_path(stage, destination)
    assert file_publication.staged_file_destination_key(stage, purpose="other") is None
    assert file_publication.staged_file_destination_key(stage, suffix=".json") is None
    other = tmp_path / "other"
    other.mkdir()
    assert file_publication.staged_file_destination_key(other / stage.name) == key
    assert not file_publication.is_owned_staged_file_path(
        other / stage.name, destination
    )


@pytest.mark.parametrize("text", ["", "café\n"], ids=["empty", "utf8"])
def test_unchanged_text_requires_no_publication(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, text: str
) -> None:
    path = tmp_path / "existing.txt"
    path.write_bytes(text.encode("utf-8"))
    before = path.stat()

    def forbidden(*_args, **_kwargs):
        pytest.fail("an equal direct leaf must not require publication permission")

    monkeypatch.setattr(file_publication, "atomic_write_bytes", forbidden)
    atomic_io._atomic_write_text(path, text, skip_if_unchanged=True)
    assert path.read_bytes() == text.encode("utf-8")
    assert (path.stat().st_ino, path.stat().st_mtime_ns) == (
        before.st_ino,
        before.st_mtime_ns,
    )


@pytest.mark.parametrize(
    "previous", [None, b"different", b"value\r\n", b"value", b"\xff" * 6]
)
def test_changed_text_uses_exact_bytes_and_existing_publisher(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, previous: bytes | None
) -> None:
    path = tmp_path / "generated.txt"
    if previous is not None:
        path.write_bytes(previous)
    publish = file_publication.atomic_write_bytes
    writes = []

    def record(destination, data, **kwargs):
        writes.append((destination, data))
        publish(destination, data, **kwargs)

    monkeypatch.setattr(file_publication, "atomic_write_bytes", record)
    atomic_io._write_text_if_changed(path, "value\n")
    assert writes == [(path, b"value\n")]
    assert path.read_bytes() == b"value\n"


@pytest.mark.parametrize("skip", [False, True])
def test_json_equality_is_opt_in_and_preserves_wire_bytes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, skip: bool
) -> None:
    path = tmp_path / "receipt.json"
    expected = b'{\n  "a": "caf\\u00e9",\n  "z": 1\n}\n'
    path.write_bytes(expected)
    publish = file_publication.atomic_write_bytes
    writes = []

    def record(destination, data, **kwargs):
        writes.append(data)
        publish(destination, data, **kwargs)

    monkeypatch.setattr(file_publication, "atomic_write_bytes", record)
    atomic_io._atomic_write_json(
        path, {"z": 1, "a": "café"}, sort_keys=True, skip_if_unchanged=skip
    )
    assert writes == ([] if skip else [expected])
    assert path.read_bytes() == expected


@pytest.mark.parametrize("invalid", ["durability", "directory", "indirect"])
def test_unchanged_text_cannot_bypass_publication_validation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, invalid: str
) -> None:
    path = tmp_path / "output"
    if invalid == "directory":
        path.mkdir()
    else:
        path.write_bytes(b"same")
    if invalid == "indirect":
        is_link_like = file_publication.is_link_like
        monkeypatch.setattr(
            file_publication,
            "is_link_like",
            lambda candidate: candidate == path or is_link_like(candidate),
        )
    policy = (
        "invalid"
        if invalid == "durability"
        else file_publication.PublicationDurability.DURABLE
    )
    with pytest.raises(ValueError):
        atomic_io._atomic_write_text(
            path, "same", durability=policy, skip_if_unchanged=True
        )
    if invalid != "directory":
        assert path.read_bytes() == b"same"


def test_unchanged_text_size_mismatch_does_not_read_existing_extent(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = tmp_path / "output"
    path.write_bytes(b"longer than requested")

    def forbidden(*_args, **_kwargs):
        pytest.fail("a size mismatch must publish without traversing old contents")

    monkeypatch.setattr(atomic_io, "iter_stable_regular_file_chunks", forbidden)
    atomic_io._atomic_write_text(path, "new", skip_if_unchanged=True)
    assert path.read_bytes() == b"new"


@pytest.mark.parametrize(
    "failure", [OSError("read denied"), atomic_io.StableRegularFileError("unstable")]
)
@pytest.mark.parametrize("publish_fails", [False, True])
def test_unchanged_text_read_failure_uses_publisher_and_preserves_its_failure(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    failure: Exception,
    publish_fails: bool,
) -> None:
    path = tmp_path / "output"
    path.write_bytes(b"same")
    publish = file_publication.atomic_write_bytes
    primary = OSError("publication denied")
    calls = []

    def unreadable(*_args, **_kwargs):
        raise failure

    def record(destination, data, **kwargs):
        calls.append(data)
        if publish_fails:
            raise primary
        publish(destination, data, **kwargs)

    monkeypatch.setattr(atomic_io, "open_stable_regular_file", unreadable)
    monkeypatch.setattr(file_publication, "atomic_write_bytes", record)
    if publish_fails:
        with pytest.raises(OSError) as caught:
            atomic_io._atomic_write_text(path, "same", skip_if_unchanged=True)
        assert caught.value is primary
    else:
        atomic_io._atomic_write_text(path, "same", skip_if_unchanged=True)
    assert calls == [b"same"]
    assert path.read_bytes() == b"same"


@pytest.mark.parametrize(
    "mutation", ["same-size-restored-mtime", "growth", "replacement"]
)
def test_unchanged_text_closing_fence_cannot_justify_skipping_publication(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, mutation: str
) -> None:
    from contextlib import contextmanager

    path = tmp_path / "output"
    path.write_bytes(b"same")
    before = path.stat()
    stable_open = atomic_io.open_stable_regular_file
    publish = file_publication.atomic_write_bytes
    writes = []
    attempts = []

    @contextmanager
    def mutate_after_comparison(candidate, **kwargs):
        with stable_open(candidate, **kwargs) as opened:
            yield opened
            attempts.append(mutation)
            # POSIX allows this mutation and the real closing fence must catch
            # it. Windows denies it while the reader owns its descriptor; that
            # read failure must also fall through after descriptor closure.
            if mutation == "replacement":
                replacement = tmp_path / "replacement"
                replacement.write_bytes(b"evil")
                replacement.replace(path)
            else:
                path.write_bytes(
                    b"evil" if mutation == "same-size-restored-mtime" else b"grown!"
                )
            os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))

    def record(destination, data, **kwargs):
        writes.append(data)
        publish(destination, data, **kwargs)

    monkeypatch.setattr(atomic_io, "open_stable_regular_file", mutate_after_comparison)
    monkeypatch.setattr(file_publication, "atomic_write_bytes", record)
    atomic_io._atomic_write_text(path, "same", skip_if_unchanged=True)
    assert attempts == [mutation]
    assert writes == [b"same"]
    assert path.read_bytes() == b"same"


def test_unchanged_text_comparison_has_a_fixed_read_extent(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = tmp_path / "output"
    text = "a" * (2 * 1024 * 1024 + 17)
    path.write_bytes(text.encode("utf-8"))
    chunks = atomic_io.iter_stable_regular_file_chunks
    sizes = []

    def observe(opened):
        for chunk in chunks(opened):
            sizes.append(len(chunk))
            yield chunk

    def forbidden(*_args, **_kwargs):
        pytest.fail("bounded equal traversal must not publish")

    monkeypatch.setattr(atomic_io, "iter_stable_regular_file_chunks", observe)
    monkeypatch.setattr(file_publication, "atomic_write_bytes", forbidden)
    atomic_io._atomic_write_text(path, text, skip_if_unchanged=True)
    assert sum(sizes) == len(text)
    assert max(sizes) <= 256 * 1024
    assert len(sizes) > 1


def test_unchanged_text_interrupt_is_not_reclassified_as_a_comparison_miss(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = tmp_path / "output"
    path.write_bytes(b"same")
    primary = KeyboardInterrupt("interrupted comparison")

    def interrupt(*_args, **_kwargs):
        raise primary

    def forbidden(*_args, **_kwargs):
        pytest.fail("comparison interruption must not launch publication")

    monkeypatch.setattr(atomic_io, "open_stable_regular_file", interrupt)
    monkeypatch.setattr(file_publication, "atomic_write_bytes", forbidden)
    with pytest.raises(KeyboardInterrupt) as caught:
        atomic_io._atomic_write_text(path, "same", skip_if_unchanged=True)
    assert caught.value is primary
    assert path.read_bytes() == b"same"
