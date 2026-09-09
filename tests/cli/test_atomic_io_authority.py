from __future__ import annotations

import errno
import hashlib
import stat
from pathlib import Path
import zipfile

import pytest

from molt.cli import atomic_io
from molt.cli import backend_cache
from molt.cli.backend_artifact_contract import (
    BackendArtifactContract,
    BackendArtifactKind,
)
from molt import file_publication
from molt.cli.runtime_wasm_generation import publish_runtime_wasm_generation
from molt.wasm_artifact import (
    _build_wasm_sections,
    _write_wasm_string,
    transform_wasm_publication_file,
)
from tests.runtime_build_identity_helper import runtime_build_identity


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
    copyfile = atomic_io.shutil.copyfile

    def changed_copy(src: Path, staged: Path) -> None:
        copyfile(src, staged)
        staged.write_bytes(b"changed during copy")

    monkeypatch.setattr(atomic_io.shutil, "copyfile", changed_copy)
    with pytest.raises(ValueError, match="source changed while staging verified copy"):
        atomic_io._atomic_copy_file(source, destination, expected_sha256=expected)
    assert not list(tmp_path.glob(".*.tmp"))
    if existing:
        assert destination.read_bytes() == b"previous"
    else:
        assert not destination.exists()
    monkeypatch.setattr(atomic_io.shutil, "copyfile", copyfile)
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

            monkeypatch.setattr(atomic_io.os, "link", no_links)
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
    monkeypatch.setattr(atomic_io.os, "fsync", lambda fd: calls.append(fd))
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
    # Immutable shared+reloc members plus the atomic pair pointer. Internal
    # publication deliberately creates no fixed-name compatibility projections.
    assert len(calls) == 3 * barriers_per_commit

    calls.clear()
    wasm = tmp_path / "publication.wasm"
    wasm.write_bytes(
        _build_wasm_sections([(0, _write_wasm_string(".debug_info") + b"debug")])
    )
    transform_wasm_publication_file(
        wasm, rename_map={}, final_artifact=True, preserve_debug=False
    )
    assert len(calls) == barriers_per_commit


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

    monkeypatch.setattr(file_publication.time, "sleep", lambda _seconds: None)

    file_publication.windows_replace_write_through(
        tmp_path / "stage",
        tmp_path / "destination",
        replace_once=transient,
    )

    assert calls == 2


def test_readonly_copy_applies_final_mode_after_durability(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "readonly-source"
    destination = tmp_path / "destination"
    source.write_bytes(b"immutable")
    source.chmod(stat.S_IREAD)
    original = file_publication.durable_replace
    staged_modes: list[int] = []

    def observe(staged: Path, target: Path) -> None:
        staged_modes.append(staged.stat().st_mode)
        original(staged, target)

    monkeypatch.setattr(file_publication, "durable_replace", observe)
    atomic_io._atomic_copy_file(source, destination)

    assert staged_modes[0] & stat.S_IWRITE
    assert not destination.stat().st_mode & stat.S_IWRITE
    assert destination.read_bytes() == b"immutable"

    replacement = tmp_path / "readonly-replacement"
    replacement.write_bytes(b"replacement")
    replacement.chmod(stat.S_IREAD)
    atomic_io._atomic_copy_file(replacement, destination)
    assert destination.read_bytes() == b"replacement"
    assert not destination.stat().st_mode & stat.S_IWRITE
