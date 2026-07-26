from __future__ import annotations

import contextlib
from contextlib import contextmanager
import ctypes
import errno
import json
import os
import shutil
import stat
import time
import uuid
from pathlib import Path
from typing import Any, Iterator, Mapping
import zipfile


_MOVEFILE_REPLACE_EXISTING = 0x1
_MOVEFILE_WRITE_THROUGH = 0x8
_WINDOWS_REPLACE_RETRY_ERRORS = frozenset({5, 32, 33})


def _move_file_ex_write_through(
    staged: Path,
    destination: Path,
    *,
    move_file_ex: Any | None = None,
    get_last_error: Any | None = None,
) -> None:
    """Commit one Windows namespace change with write-through durability."""

    if move_file_ex is None:
        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        move_file_ex = kernel32.MoveFileExW
        move_file_ex.argtypes = (ctypes.c_wchar_p, ctypes.c_wchar_p, ctypes.c_uint32)
        move_file_ex.restype = ctypes.c_int
        get_last_error = ctypes.get_last_error
    flags = _MOVEFILE_REPLACE_EXISTING | _MOVEFILE_WRITE_THROUGH
    if not move_file_ex(os.fspath(staged), os.fspath(destination), flags):
        assert get_last_error is not None
        raise ctypes.WinError(get_last_error())


def _windows_replace_write_through(
    staged: Path,
    destination: Path,
    *,
    replace_once: Any = _move_file_ex_write_through,
) -> None:
    for attempt in range(25):
        try:
            replace_once(staged, destination)
            return
        except OSError as exc:
            if getattr(exc, "winerror", None) not in _WINDOWS_REPLACE_RETRY_ERRORS:
                raise
            if attempt == 24:
                raise
            time.sleep(0.01)


def _namespace_replace_once(staged: Path, destination: Path) -> None:
    if os.name == "nt":
        _windows_replace_write_through(staged, destination)
    else:
        os.replace(staged, destination)


def _durable_replace(staged: Path, destination: Path) -> None:
    """One cross-platform durable staged-file commit authority.

    POSIX publishes with a staged-file fsync, atomic replace, and destination
    directory fsync. Windows publishes with a staged-file fsync followed by
    ``MoveFileExW(REPLACE_EXISTING | WRITE_THROUGH)``. These are the sole
    platform durability contracts for every file publication in this module.
    """

    # Windows' CRT rejects fsync on a read-only descriptor (EBADF), so reopen
    # the completed stage read/write for the sole content barrier.
    with staged.open("r+b") as handle:
        os.fsync(handle.fileno())
    replaced_mode: int | None = None
    try:
        if destination.exists() and not destination.stat().st_mode & stat.S_IWRITE:
            replaced_mode = destination.stat().st_mode
            destination.chmod(replaced_mode | stat.S_IWRITE)
        _namespace_replace_once(staged, destination)
    except BaseException:
        if replaced_mode is not None and destination.exists():
            with contextlib.suppress(OSError):
                destination.chmod(replaced_mode)
        raise
    if os.name == "posix":
        unsupported = {
            errno.EBADF,
            errno.EINVAL,
            errno.ENOTSUP,
            getattr(errno, "EOPNOTSUPP", errno.ENOTSUP),
        }
        try:
            dir_fd = os.open(destination.parent, os.O_RDONLY)
        except OSError as exc:
            if exc.errno in unsupported:
                return
            raise
        try:
            try:
                os.fsync(dir_fd)
            except OSError as exc:
                if exc.errno not in unsupported:
                    raise
        finally:
            os.close(dir_fd)


def _atomic_write_text(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp_path = path.with_name(f".{path.name}.{os.getpid()}.{uuid.uuid4().hex}.tmp")
    try:
        with tmp_path.open("w", encoding="utf-8") as handle:
            handle.write(text)
        _durable_replace(tmp_path, path)
    finally:
        with contextlib.suppress(OSError):
            if tmp_path.exists():
                tmp_path.unlink()


def _write_text_if_changed(path: Path, content: str) -> None:
    try:
        existing = path.read_text()
    except OSError:
        existing = None
    if existing == content:
        return
    _atomic_write_text(path, content)


def _remove_file_or_tree(path: Path) -> None:
    if path.is_dir() and not path.is_symlink():
        shutil.rmtree(path)
    else:
        path.unlink()


def _atomic_write_bytes(path: Path, data: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp_path = path.with_name(f".{path.name}.{os.getpid()}.{uuid.uuid4().hex}.tmp")
    try:
        with tmp_path.open("wb") as handle:
            handle.write(data)
        _durable_replace(tmp_path, path)
    finally:
        with contextlib.suppress(OSError):
            if tmp_path.exists():
                tmp_path.unlink()


def _atomic_write_json(
    path: Path,
    payload: Any,
    *,
    indent: int | None = 2,
    sort_keys: bool = False,
    default: Any | None = None,
) -> None:
    _atomic_write_text(
        path,
        json.dumps(
            payload,
            indent=indent,
            sort_keys=sort_keys,
            default=default,
        )
        + "\n",
    )


def _write_json_sidecar(path: Path, payload: Mapping[str, Any]) -> None:
    _atomic_write_json(path, payload, indent=2, sort_keys=True)


def _codesign_atomic_copy_temp(path: Path) -> None:
    from molt.cli.native_toolchain import _codesign_binary

    _codesign_binary(path)


def _atomic_copy_file(src: Path, dst: Path, *, codesign: bool = False) -> None:
    dst.parent.mkdir(parents=True, exist_ok=True)
    tmp_path = dst.with_name(f".{dst.name}.{os.getpid()}.{uuid.uuid4().hex}.tmp")
    try:
        shutil.copyfile(src, tmp_path)
        if codesign:
            _codesign_atomic_copy_temp(tmp_path)
        _durable_replace(tmp_path, dst)
        # A read-only source must not make the staged file impossible to fsync.
        # Final metadata belongs after the content and namespace durability barrier.
        shutil.copymode(src, dst)
    finally:
        with contextlib.suppress(OSError):
            if tmp_path.exists():
                tmp_path.unlink()


# os.link is only an optimization over copying. When it fails because hard
# links are unavailable for this (src, dst) pair we must fall back to a byte
# copy: cross-device (EXDEV), permission (EPERM/EACCES), or a filesystem without
# hard-link support. exFAT/FAT volumes can reject
# os.link with ERROR_INVALID_FUNCTION (winerror 1 -> errno EINVAL 22), which is
# NONE of the classic POSIX link errnos, so it must be recognized explicitly or
# every freshly staged artifact is dropped on that volume.
_LINK_COPY_FALLBACK_ERRNOS = frozenset(
    {errno.EXDEV, errno.EPERM, errno.EACCES, errno.ENOTSUP, errno.EINVAL, errno.ENOENT}
)
# ERROR_INVALID_FUNCTION / ERROR_NOT_SUPPORTED / ERROR_INVALID_PARAMETER.
_LINK_COPY_FALLBACK_WINERRORS = frozenset({1, 50, 87})


def _link_failure_wants_copy(exc: OSError) -> bool:
    """True when an ``os.link`` OSError means "no hard links here — copy instead"."""
    if exc.errno in _LINK_COPY_FALLBACK_ERRNOS:
        return True
    return getattr(exc, "winerror", None) in _LINK_COPY_FALLBACK_WINERRORS


def _atomic_link_or_copy_file(src: Path, dst: Path) -> None:
    dst.parent.mkdir(parents=True, exist_ok=True)
    tmp_path = dst.with_name(f".{dst.name}.{os.getpid()}.{uuid.uuid4().hex}.tmp")
    try:
        source_mode = src.stat().st_mode
        if source_mode & stat.S_IWRITE:
            try:
                os.link(src, tmp_path)
                _durable_replace(tmp_path, dst)
                return
            except OSError as exc:
                if not _link_failure_wants_copy(exc):
                    raise
        _atomic_copy_file(src, dst)
    finally:
        with contextlib.suppress(OSError):
            if tmp_path.exists():
                tmp_path.unlink()


@contextmanager
def _atomic_zip_file(path: Path) -> Iterator[zipfile.ZipFile]:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp_path = path.with_name(f".{path.name}.{os.getpid()}.{uuid.uuid4().hex}.tmp")
    try:
        with zipfile.ZipFile(tmp_path, "w") as zf:
            yield zf
        _durable_replace(tmp_path, path)
    finally:
        with contextlib.suppress(OSError):
            if tmp_path.exists():
                tmp_path.unlink()
