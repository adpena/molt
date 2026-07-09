from __future__ import annotations

from pathlib import Path

from molt.cli import file_hashing


def test_source_fingerprint_files_are_deterministic_and_filtered(tmp_path: Path) -> None:
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
