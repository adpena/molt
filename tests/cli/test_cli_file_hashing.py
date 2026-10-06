from __future__ import annotations

import os
from pathlib import Path

import pytest

from molt import file_hashing


def test_sha256_primitives_share_file_hashing_authority(tmp_path: Path) -> None:
    path = tmp_path / "content.bin"
    path.write_bytes(b"abc")
    expected = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"

    assert file_hashing._sha256_bytes(b"abc") == expected
    assert file_hashing._sha256_file_with_size(path) == (expected, 3)
    assert file_hashing._sha256_file(path) == expected


def test_source_fingerprint_files_are_deterministic_and_filtered(
    tmp_path: Path,
) -> None:
    root = tmp_path / "repo"
    source_root = root / "src"
    pycache = source_root / "__pycache__"
    pycache.mkdir(parents=True)
    (source_root / "b.rs").write_text("pub fn b() {}\n", encoding="utf-8")
    (source_root / "a.py").write_text("print('a')\n", encoding="utf-8")
    (source_root / "z.pyc").write_bytes(b"bytecode")
    (pycache / "a.pyc").write_bytes(b"bytecode")

    files = [
        path.relative_to(root).as_posix()
        for path in file_hashing._source_fingerprint_files(source_root)
    ]

    assert files == ["src/a.py", "src/b.rs"]
    metadata = file_hashing._hash_source_tree_metadata([source_root], root)
    assert metadata is not None
    assert metadata[1] == 2


def test_content_change_time_path_and_descriptor_agree_for_one_generation(
    tmp_path: Path,
) -> None:
    path = tmp_path / "content.bin"
    path.write_bytes(b"before")
    metadata = path.stat()
    with path.open("rb") as handle:
        path_change = file_hashing.content_change_time_ns(path, metadata)
        handle_change = file_hashing.content_change_time_ns_from_fd(
            handle.fileno(), os.fstat(handle.fileno())
        )
    assert path_change is not None
    assert handle_change == path_change


def test_content_change_time_has_one_cross_module_authority() -> None:
    assert callable(file_hashing.content_change_time_ns)
    assert not hasattr(file_hashing, "_content_change_time_ns")


def test_windows_change_time_fails_closed_when_api_is_unavailable(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = tmp_path / "content.bin"
    path.write_bytes(b"content")
    monkeypatch.setattr(file_hashing, "_windows_file_api", lambda: None)

    assert file_hashing._windows_change_time_ns(path) is None
