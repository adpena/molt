from __future__ import annotations

import hashlib
import os
from functools import lru_cache
from pathlib import Path
from typing import Any, Callable, Iterator, Sequence


from molt.source_admission import admitted_source_files, source_directories

# A timestamp near a tick can miss another same-tick write (the racy-git rule).
DIGEST_RACY_WINDOW_NS = 2_000_000_000


def _sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


@lru_cache(maxsize=1)
def _windows_file_api() -> tuple[Any, Any, Any] | None:
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


def open_stable_read_descriptor(path: Path) -> int:
    """Open the shared direct-file reader, excluding Windows writes/deletion.

    Windows ChangeTime is not a unique write sequence: distinct writes can
    share one clock tick. Keep the admitted file under read-only sharing until
    its caller closes the descriptor. POSIX keeps its no-follow open and native
    before/after mutation checks in the owning stable-file transaction.
    """
    flags = (
        os.O_RDONLY
        | getattr(os, "O_BINARY", 0)
        | getattr(os, "O_NOINHERIT", 0)
        | getattr(os, "O_NOFOLLOW", 0)
    )
    if os.name != "nt":
        return os.open(path, flags)
    api = _windows_file_api()
    if api is None:
        raise OSError("Windows direct-file handle API is unavailable")
    import msvcrt

    ctypes, kernel32, _file_basic_info = api
    handle = kernel32.CreateFileW(
        str(path),
        0x80000000,  # GENERIC_READ
        0x00000001,  # FILE_SHARE_READ; writes and deletion remain excluded.
        None,
        3,  # OPEN_EXISTING
        0x00200000,  # FILE_FLAG_OPEN_REPARSE_POINT; never follow a swapped link.
        None,
    )
    if handle in (None, ctypes.c_void_p(-1).value):
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        # Ownership transfers to the CRT descriptor only after this succeeds.
        return msvcrt.open_osfhandle(handle, flags)
    except BaseException:
        kernel32.CloseHandle(handle)
        raise


def _windows_change_time_ns(path: Path) -> int | None:
    api = _windows_file_api()
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
    api = _windows_file_api()
    if api is None:
        return None
    ctypes, kernel32, file_basic_info = api
    try:
        info = file_basic_info()
        if not kernel32.GetFileInformationByHandleEx(
            handle, 0, ctypes.byref(info), ctypes.sizeof(info)
        ):
            return None
        # FILE_BASIC_INFO uses 100 ns ticks since 1601; every public
        # content-change token shares stat/time.time_ns Unix-epoch units.
        return (int(info.ChangeTime) - 116_444_736_000_000_000) * 100
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


def _iter_source_fingerprint_files(
    path: Path, *, on_directory: Callable[[Path], None] | None = None
) -> Iterator[Path]:
    files = admitted_source_files((path,))
    if on_directory is not None:
        for directory in source_directories(files, (path,)):
            on_directory(directory)
    yield from files


def _source_fingerprint_files(path: Path) -> list[Path]:
    return list(_iter_source_fingerprint_files(path))


def _hash_source_tree_file(path: Path, root: Path, hasher: Any) -> None:
    rel_bytes = _source_fingerprint_label(path, root).encode("utf-8")
    hasher.update(rel_bytes)
    hasher.update(b"\0")
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            hasher.update(chunk)
    hasher.update(b"\0")


def _source_fingerprint_label(path: Path, root: Path) -> str:
    # Admission already establishes component containment. Path.relative_to
    # scans/constructs every ancestor; avoid repeating that
    # work for each file in a closure. normcase retains Windows path semantics.
    text = os.fspath(path)
    prefix = os.fspath(root).rstrip(os.sep) + os.sep
    if os.path.normcase(text[: len(prefix)]) == os.path.normcase(prefix):
        return text[len(prefix) :]
    try:
        return str(path.relative_to(root))
    except ValueError:
        return text


def _hash_source_tree_paths(
    paths: Sequence[Path],
    root: Path,
    hasher: Any,
) -> None:
    for item in admitted_source_files(paths):
        _hash_source_tree_file(item, root, hasher)


def _hash_source_tree_metadata(
    paths: Sequence[Path],
    root: Path,
) -> tuple[str, int] | None:
    hasher = hashlib.sha256()
    file_count = 0
    try:
        for item in admitted_source_files(paths):
            try:
                stat = item.stat()
            except OSError:
                return None
            change_time_ns = content_change_time_ns(item, stat)
            if change_time_ns is None:
                return None
            rel_text = _source_fingerprint_label(item, root)
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
