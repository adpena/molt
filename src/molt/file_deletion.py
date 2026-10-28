"""Canonical filesystem deletion primitive for Molt-owned artifact cleanup.

Callers remain responsible for proving that a path is in their deletion scope.
This module owns the cross-platform mechanics: do not follow directory links,
tolerate an already-absent path, and delete Windows read-only links without
changing the underlying file's attributes. Windows requires a filesystem with
FileDispositionInfoEx POSIX/IGNORE_READONLY support (Windows 10 1809 or newer).
Unsupported operations fail closed; there is no attribute-changing fallback.
POSIX directory seals are removed only within an explicit caller-owned root;
file modes and external link targets are never changed during deletion.
"""

from __future__ import annotations

import errno
import os
import shutil
import stat
from functools import lru_cache
from pathlib import Path
from typing import Any, Callable


@lru_cache(maxsize=1)
def _windows_delete_api() -> tuple[Any, Any, Any]:
    if os.name != "nt":
        raise OSError("Windows deletion API is unavailable on this platform")
    import ctypes
    from ctypes import wintypes

    class FileAttributeTagInfo(ctypes.Structure):
        _fields_ = [
            ("FileAttributes", wintypes.DWORD),
            ("ReparseTag", wintypes.DWORD),
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
    kernel32.SetFileInformationByHandle.restype = wintypes.BOOL
    kernel32.SetFileInformationByHandle.argtypes = [
        wintypes.HANDLE,
        ctypes.c_int,
        ctypes.c_void_p,
        wintypes.DWORD,
    ]
    kernel32.CloseHandle.restype = wintypes.BOOL
    kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
    return ctypes, kernel32, FileAttributeTagInfo


def _windows_delete_leaf(path: Path, *, directory: bool) -> None:
    """Remove the opened link without following it or changing file attributes."""
    spelling = os.fspath(path)
    if "\0" in spelling:
        raise ValueError("embedded null character")
    ctypes, kernel32, file_attribute_tag_info = _windows_delete_api()

    def error_from_last_error() -> OSError:
        error = ctypes.WinError(ctypes.get_last_error())
        error.filename = spelling
        return error

    handle = kernel32.CreateFileW(
        spelling,
        0x00010000 | 0x0080 | 0x0100,  # DELETE | READ/WRITE_ATTRIBUTES
        0x00000001 | 0x00000002 | 0x00000004,  # SHARE_READ/WRITE/DELETE
        None,
        3,  # OPEN_EXISTING
        0x02000000 | 0x00200000,  # BACKUP_SEMANTICS | OPEN_REPARSE_POINT
        None,
    )
    if handle in (None, ctypes.c_void_p(-1).value):
        raise error_from_last_error()
    failure: BaseException | None = None
    try:
        metadata = file_attribute_tag_info()
        if not kernel32.GetFileInformationByHandleEx(
            handle, 9, ctypes.byref(metadata), ctypes.sizeof(metadata)
        ):  # FileAttributeTagInfo
            raise error_from_last_error()
        is_directory = bool(metadata.FileAttributes & 0x10)
        is_reparse = bool(metadata.FileAttributes & 0x400)
        if directory and not is_directory:
            raise NotADirectoryError(errno.ENOTDIR, os.strerror(errno.ENOTDIR), path)
        if not directory and is_directory and not is_reparse:
            raise IsADirectoryError(errno.EISDIR, os.strerror(errno.EISDIR), path)
        # FILE_DISPOSITION_INFO_EX contains one DWORD Flags. Unlike chmod,
        # IGNORE_READONLY does not alter attributes shared by other hardlinks.
        disposition = ctypes.c_uint32(0x01 | 0x02 | 0x10)
        if not kernel32.SetFileInformationByHandle(
            handle, 21, ctypes.byref(disposition), ctypes.sizeof(disposition)
        ):  # FileDispositionInfoEx: DELETE | POSIX_SEMANTICS | IGNORE_READONLY
            raise error_from_last_error()
    except BaseException as error:
        failure = error
        raise
    finally:
        if not kernel32.CloseHandle(handle):
            error = error_from_last_error()
            if failure is None:
                raise error
            failure.add_note(f"closing deletion handle also failed: {error}")


def _rmtree_error(
    operation: Callable[..., object], raw_path: str, error: BaseException
) -> None:
    """Handle failed deletion only; never turn an enumeration error into a delete."""
    if isinstance(error, FileNotFoundError):
        return
    if (
        os.name != "nt"
        or not isinstance(error, PermissionError)
        or operation not in (os.unlink, os.rmdir)
    ):
        raise error
    try:
        _windows_delete_leaf(Path(raw_path), directory=operation is os.rmdir)
    except FileNotFoundError:
        pass


def unlink_file(path: Path) -> None:
    """Remove one owned leaf, including read-only files; never recurse."""
    try:
        path.unlink(missing_ok=True)
    except PermissionError:
        if os.name != "nt":
            raise
        try:
            _windows_delete_leaf(path, directory=False)
        except FileNotFoundError:
            pass


def _recursive_deletion_path(path: Path, metadata: os.stat_result) -> Path:
    """Keep enumerated Windows child names in their actual filesystem namespace.

    An ordinary root may contain verbatim-only children. Starting rmtree with
    that root's verbatim spelling keeps trailing dots/spaces in every DirEntry
    path; an ordinary child spelling could address a different sibling instead.
    Do not resolve symlinks or change a caller's explicit verbatim coordinate.
    """
    if os.name != "nt":
        return path
    spelling = os.fspath(path)
    if spelling.startswith("\\\\?\\"):
        return path
    absolute = os.path.abspath(spelling)
    if absolute.startswith("\\\\"):
        verbatim = Path("\\\\?\\UNC\\" + absolute[2:])
    else:
        verbatim = Path("\\\\?\\" + absolute)
    if not os.path.samestat(metadata, verbatim.lstat()):
        raise OSError("Windows recursive deletion spelling changed its entry")
    return verbatim


def _unseal_owned_directories(path: Path, metadata: os.stat_result) -> None:
    """Restore only directory owner-write, through non-following descriptors.

    POSIX unlink needs write permission on the containing directory, not the
    file. Never chmod files: their attributes may be shared by external links.
    The caller has already checked the explicit owning root.
    """
    descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        if not os.path.samestat(metadata, os.fstat(descriptor)):
            raise OSError("owned deletion directory changed before unsealing")
        for _directory, _children, _files, directory_fd in os.fwalk(
            ".", dir_fd=descriptor, follow_symlinks=False
        ):
            current = os.fstat(directory_fd)
            if not current.st_mode & stat.S_IWUSR:
                os.fchmod(directory_fd, stat.S_IMODE(current.st_mode) | stat.S_IWUSR)
    finally:
        os.close(descriptor)


def delete_path(path: Path, *, owning_root: Path | None = None) -> tuple[bool, str]:
    """Delete an authorized path; unseal directories only with explicit custody.

    Supplying an owner requires a strict physical descendant. Existing callers
    without that grant retain ordinary deletion and never acquire chmod rights.
    Eligibility, lifetime locks and identity checks still belong to the caller.
    """
    try:
        if owning_root is not None:
            from molt.file_publication import resolve_owned_path

            owner = resolve_owned_path(owning_root)
            if path.name in {"", ".", ".."}:
                raise ValueError(f"deletion requires an owned leaf: {path}")
            # A leaf link may be unlinked, but neither it nor any ancestor may
            # redirect the permission changes into another tree.
            path = resolve_owned_path(path.parent) / path.name
            if path == owner or not path.is_relative_to(owner):
                raise ValueError(f"deletion outside owning root: {path}")
        try:
            metadata = path.lstat()
        except FileNotFoundError:
            return True, ""
        if stat.S_ISDIR(metadata.st_mode) and not (
            getattr(metadata, "st_file_attributes", 0) & 0x400
        ):
            if owning_root is not None and os.name == "posix":
                _unseal_owned_directories(path, metadata)
            shutil.rmtree(_recursive_deletion_path(path, metadata), onexc=_rmtree_error)
        else:
            unlink_file(path)
        return True, ""
    except (OSError, ValueError) as error:
        return False, str(error)
