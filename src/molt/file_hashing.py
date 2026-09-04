from __future__ import annotations

import hashlib
import os
from functools import lru_cache
from pathlib import Path
from typing import Any, Iterator, Sequence


_SOURCE_FINGERPRINT_IGNORED_DIRS = frozenset({"__pycache__"})
_SOURCE_FINGERPRINT_IGNORED_SUFFIXES = frozenset({".pyc", ".pyo"})


def _sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


@lru_cache(maxsize=1)
def _windows_change_time_api() -> tuple[Any, Any, Any] | None:
    if os.name != "nt":
        return None
    try:
        import ctypes
        from ctypes import wintypes

        class FileBasicInfo(ctypes.Structure):
            _fields_ = [
                ("CreationTime", ctypes.c_longlong),
                ("LastAccessTime", ctypes.c_longlong),
                ("LastWriteTime", ctypes.c_longlong),
                ("ChangeTime", ctypes.c_longlong),
                ("FileAttributes", wintypes.DWORD),
            ]

        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel32.CreateFileW.restype = wintypes.HANDLE
        kernel32.CreateFileW.argtypes = [
            wintypes.LPCWSTR,
            wintypes.DWORD,
            wintypes.DWORD,
            ctypes.c_void_p,
            wintypes.DWORD,
            wintypes.DWORD,
            wintypes.HANDLE,
        ]
        kernel32.GetFileInformationByHandleEx.restype = wintypes.BOOL
        kernel32.GetFileInformationByHandleEx.argtypes = [
            wintypes.HANDLE,
            ctypes.c_int,
            ctypes.c_void_p,
            wintypes.DWORD,
        ]
        kernel32.CloseHandle.restype = wintypes.BOOL
        kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
        return ctypes, kernel32, FileBasicInfo
    except (AttributeError, OSError, ValueError):
        return None


def _windows_change_time_ns(path: Path) -> int | None:
    api = _windows_change_time_api()
    if api is None:
        return None
    ctypes, kernel32, file_basic_info = api
    try:
        handle = kernel32.CreateFileW(
            str(path),
            0x0080,
            0x00000001 | 0x00000002 | 0x00000004,
            None,
            3,
            0x02000000,
            None,
        )
        invalid_handle = ctypes.c_void_p(-1).value
        if handle in (None, invalid_handle):
            return None
        try:
            return _windows_handle_change_time_ns(handle)
        finally:
            kernel32.CloseHandle(handle)
    except (AttributeError, OSError, ValueError):
        return None


def content_change_time_ns(path: Path, stat: os.stat_result) -> int | None:
    if os.name == "nt":
        # Windows st_ctime is creation time and is never a substitute for the
        # NTFS ChangeTime query. None tells exact consumers to fail closed and
        # metadata caches to fall back to content hashing.
        return _windows_change_time_ns(path)
    return stat.st_ctime_ns


def _windows_handle_change_time_ns(handle: Any) -> int | None:
    api = _windows_change_time_api()
    if api is None:
        return None
    ctypes, kernel32, file_basic_info = api
    try:
        info = file_basic_info()
        if not kernel32.GetFileInformationByHandleEx(
            handle, 0, ctypes.byref(info), ctypes.sizeof(info)
        ):
            return None
        return int(info.ChangeTime) * 100
    except (AttributeError, OSError, ValueError):
        return None


def content_change_time_ns_from_fd(
    file_descriptor: int,
    stat: os.stat_result,
) -> int | None:
    """Return content-change time from the already-open file when supported."""

    if os.name != "nt":
        return stat.st_ctime_ns
    try:
        import msvcrt

        return _windows_handle_change_time_ns(msvcrt.get_osfhandle(file_descriptor))
    except (ImportError, OSError, ValueError):
        return None


def _sha256_file_with_size(path: Path) -> tuple[str, int]:
    digest = hashlib.sha256()
    size = 0
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
            size += len(chunk)
    return digest.hexdigest(), size


def _sha256_file(path: Path) -> str:
    with path.open("rb") as handle:
        return hashlib.file_digest(handle, "sha256").hexdigest()


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
                change_time_ns = content_change_time_ns(item, stat)
                if change_time_ns is None:
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
                hasher.update(str(change_time_ns).encode("utf-8"))
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
