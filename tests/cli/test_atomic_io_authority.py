from __future__ import annotations

import hashlib
import json
import stat
from pathlib import Path

import pytest

from molt.cli import atomic_io
from molt.cli.runtime_build_identity import RuntimeBuildIdentity
from molt.cli.runtime_wasm_generation import publish_runtime_wasm_generation
from molt.wasm_artifact import (
    _build_wasm_sections,
    _write_wasm_string,
    transform_wasm_publication_file,
)


def _digest(value: object) -> str:
    return hashlib.sha256(
        json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()


def _identity(kind: str, pair: dict[str, object]) -> RuntimeBuildIdentity:
    payload = {"pair": pair, "member": {"kind": kind}}
    return RuntimeBuildIdentity(_digest(payload), _digest(pair), payload)


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
    pair = {"schema": "molt.runtime-build-pair.v2", "plan": "exact"}
    publish_runtime_wasm_generation(
        shared,
        reloc,
        shared_identity=_identity("shared", pair),
        reloc_identity=_identity("reloc", pair),
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

    atomic_io._move_file_ex_write_through(
        staged,
        destination,
        move_file_ex=fake_move_file_ex,
    )

    assert calls == [
        (
            str(staged),
            str(destination),
            atomic_io._MOVEFILE_REPLACE_EXISTING | atomic_io._MOVEFILE_WRITE_THROUGH,
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

    monkeypatch.setattr(atomic_io.time, "sleep", lambda _seconds: None)

    atomic_io._windows_replace_write_through(
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
    original = atomic_io._durable_replace
    staged_modes: list[int] = []

    def observe(staged: Path, target: Path) -> None:
        staged_modes.append(staged.stat().st_mode)
        original(staged, target)

    monkeypatch.setattr(atomic_io, "_durable_replace", observe)
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
