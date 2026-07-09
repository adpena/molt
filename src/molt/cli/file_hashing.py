from __future__ import annotations

import hashlib
import os
from pathlib import Path
from typing import Any, Iterator, Sequence


_SOURCE_FINGERPRINT_IGNORED_DIRS = frozenset({"__pycache__"})
_SOURCE_FINGERPRINT_IGNORED_SUFFIXES = frozenset({".pyc", ".pyo"})


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _source_fingerprint_should_skip(path: Path) -> bool:
    return (
        any(part in _SOURCE_FINGERPRINT_IGNORED_DIRS for part in path.parts)
        or path.suffix in _SOURCE_FINGERPRINT_IGNORED_SUFFIXES
    )


def _iter_source_fingerprint_files(path: Path) -> Iterator[Path]:
    if path.is_dir():
        yield from _iter_source_fingerprint_dir(path)
        return
    if path.exists() and path.is_file() and not _source_fingerprint_should_skip(path):
        yield path


def _iter_source_fingerprint_dir(path: Path) -> Iterator[Path]:
    try:
        with os.scandir(path) as iterator:
            entries = sorted(iterator, key=lambda entry: entry.name)
    except OSError:
        raise
    for entry in entries:
        entry_path = Path(entry.path)
        if entry.name in _SOURCE_FINGERPRINT_IGNORED_DIRS:
            continue
        try:
            if entry.is_dir(follow_symlinks=False):
                yield from _iter_source_fingerprint_dir(entry_path)
            elif entry.is_file() and not _source_fingerprint_should_skip(entry_path):
                yield entry_path
        except OSError:
            raise


def _source_fingerprint_files(path: Path) -> list[Path]:
    return list(_iter_source_fingerprint_files(path))


def _hash_source_tree_file(path: Path, root: Path, hasher: Any) -> None:
    try:
        rel_path = path.relative_to(root)
        rel_bytes = str(rel_path).encode("utf-8")
    except ValueError:
        rel_bytes = str(path).encode("utf-8")
    hasher.update(rel_bytes)
    hasher.update(b"\0")
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            hasher.update(chunk)
    hasher.update(b"\0")


def _hash_source_tree_paths(
    paths: Sequence[Path],
    root: Path,
    hasher: Any,
) -> None:
    for path in sorted(paths, key=lambda item: str(item)):
        for item in _iter_source_fingerprint_files(path):
            _hash_source_tree_file(item, root, hasher)


def _hash_source_tree_metadata(
    paths: Sequence[Path],
    root: Path,
) -> tuple[str, int] | None:
    hasher = hashlib.sha256()
    file_count = 0
    try:
        for path in sorted(paths, key=lambda item: str(item)):
            for item in _iter_source_fingerprint_files(path):
                try:
                    stat = item.stat()
                except OSError:
                    return None
                try:
                    rel_path = item.relative_to(root)
                    rel_text = str(rel_path)
                except ValueError:
                    rel_text = str(item)
                hasher.update(rel_text.encode("utf-8"))
                hasher.update(b"\0")
                hasher.update(str(stat.st_size).encode("utf-8"))
                hasher.update(b"\0")
                hasher.update(str(stat.st_mtime_ns).encode("utf-8"))
                hasher.update(b"\0")
                hasher.update(str(stat.st_ctime_ns).encode("utf-8"))
                hasher.update(b"\0")
                file_count += 1
    except OSError:
        return None
    return hasher.hexdigest(), file_count


def _normalize_sha256(value: str | None) -> str | None:
    if not value:
        return None
    cleaned = value.strip().lower()
    if cleaned.startswith("sha256:"):
        cleaned = cleaned[len("sha256:") :]
    return cleaned
