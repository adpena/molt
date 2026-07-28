from __future__ import annotations

import os
from pathlib import Path

import pytest

from molt import file_hashing


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


def test_content_change_time_observes_same_size_timestamp_restored_mutation(
    tmp_path: Path,
) -> None:
    path = tmp_path / "content.bin"
    path.write_bytes(b"before")
    before_stat = path.stat()
    before = file_hashing._content_change_time_ns(path, before_stat)

    with path.open("r+b", buffering=0) as handle:
        handle.write(b"after!")
        os.fsync(handle.fileno())
    os.utime(path, ns=(before_stat.st_atime_ns, before_stat.st_mtime_ns))
    after = file_hashing._content_change_time_ns(path, path.stat())

    assert before is not None
    assert after is not None
    assert after != before


def test_windows_change_time_fails_closed_when_api_is_unavailable(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = tmp_path / "content.bin"
    path.write_bytes(b"content")
    monkeypatch.setattr(file_hashing, "_windows_change_time_api", lambda: None)

    assert file_hashing._windows_change_time_ns(path) is None
