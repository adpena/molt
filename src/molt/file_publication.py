"""Cross-platform crash-consistent publication of one filesystem leaf."""

from __future__ import annotations

from collections.abc import Callable
import contextlib
import ctypes
import errno
import hashlib
import os
from pathlib import Path
import stat
import sys
import time
from typing import Protocol
import uuid
import warnings


MOVEFILE_REPLACE_EXISTING = 0x1
MOVEFILE_WRITE_THROUGH = 0x8
WINDOWS_REPLACE_RETRY_ERRORS = frozenset({5, 32, 33})
_AT_FDCWD = -100
_RENAME_NOREPLACE = 0x1
_RENAME_EXCL = 0x4
_WINDOWS_REPARSE_POINT = getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0)


class MoveFileEx(Protocol):
    def __call__(self, source: str, destination: str, flags: int, /) -> int: ...


GetLastError = Callable[[], int]
DurableReplace = Callable[[Path, Path], None]


def _warn_after_commit(message: str) -> None:
    try:
        with warnings.catch_warnings():
            warnings.simplefilter("always", RuntimeWarning)
            warnings.warn(message, RuntimeWarning, stacklevel=3)
    except BaseException:
        pass


def _metadata_is_link_like(metadata: os.stat_result) -> bool:
    return stat.S_ISLNK(metadata.st_mode) or bool(
        getattr(metadata, "st_file_attributes", 0) & _WINDOWS_REPARSE_POINT
    )


def is_link_like(path: Path) -> bool:
    """Return whether a path is a symlink or any Windows reparse point."""

    try:
        metadata = path.lstat()
    except (FileNotFoundError, NotADirectoryError):
        return False
    return _metadata_is_link_like(metadata)


def windows_move_file_api() -> tuple[MoveFileEx, GetLastError]:
    """Resolve the Win32 namespace API only on its owning platform."""

    if sys.platform != "win32":
        raise OSError("MoveFileExW is available only on Windows")
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    move_file_ex = kernel32.MoveFileExW
    move_file_ex.argtypes = (ctypes.c_wchar_p, ctypes.c_wchar_p, ctypes.c_uint32)
    move_file_ex.restype = ctypes.c_int
    return move_file_ex, ctypes.get_last_error


def windows_error(error_code: int) -> OSError:
    if sys.platform == "win32":
        return ctypes.WinError(error_code)
    return OSError(error_code, "Windows namespace commit failed")


def move_file_ex_write_through(
    staged: Path,
    destination: Path,
    *,
    move_file_ex: MoveFileEx | None = None,
    get_last_error: GetLastError | None = None,
    replace_existing: bool = True,
) -> None:
    """Commit one Windows namespace change with write-through durability."""

    if move_file_ex is None:
        move_file_ex, get_last_error = windows_move_file_api()
    flags = MOVEFILE_WRITE_THROUGH
    if replace_existing:
        flags |= MOVEFILE_REPLACE_EXISTING
    if not move_file_ex(os.fspath(staged), os.fspath(destination), flags):
        assert get_last_error is not None
        raise windows_error(get_last_error())


def windows_replace_write_through(
    staged: Path,
    destination: Path,
    *,
    replace_once: DurableReplace = move_file_ex_write_through,
) -> None:
    for attempt in range(25):
        try:
            replace_once(staged, destination)
            return
        except OSError as exc:
            if getattr(exc, "winerror", None) not in WINDOWS_REPLACE_RETRY_ERRORS:
                raise
            if attempt == 24:
                raise
            time.sleep(0.01)


def namespace_replace_once(staged: Path, destination: Path) -> None:
    if os.name == "nt":
        windows_replace_write_through(staged, destination)
    else:
        os.replace(staged, destination)


def fsync_directory(path: Path) -> None:
    """Durably publish namespace changes on POSIX."""

    if os.name != "posix":
        return
    unsupported = {
        errno.EBADF,
        errno.EINVAL,
        errno.ENOTSUP,
        getattr(errno, "EOPNOTSUPP", errno.ENOTSUP),
    }
    try:
        directory_fd = os.open(path, os.O_RDONLY)
    except OSError as exc:
        if exc.errno in unsupported:
            return
        raise
    try:
        try:
            os.fsync(directory_fd)
        except OSError as exc:
            if exc.errno not in unsupported:
                raise
    finally:
        os.close(directory_fd)


def durable_namespace_replace(staged: Path, destination: Path) -> None:
    """Durably move one already-flushed namespace entry across platforms."""

    staged = Path(staged)
    destination = Path(destination)
    source_parent = staged.parent
    destination_parent = destination.parent
    namespace_replace_once(staged, destination)
    fsync_directory(destination_parent)
    if source_parent != destination_parent:
        fsync_directory(source_parent)


def _raise_posix_rename_error(staged: Path, destination: Path) -> None:
    error_code = ctypes.get_errno()
    raise OSError(
        error_code,
        os.strerror(error_code),
        os.fspath(staged),
        os.fspath(destination),
    )


def _linux_rename_exclusive(staged: Path, destination: Path) -> None:
    """Use Linux renameat2 so a competing destination is never replaced."""

    libc = ctypes.CDLL(None, use_errno=True)
    try:
        renameat2 = libc.renameat2
    except AttributeError as exc:
        raise OSError(
            errno.ENOSYS,
            "libc does not expose renameat2 for exclusive directory publication",
        ) from exc
    renameat2.argtypes = (
        ctypes.c_int,
        ctypes.c_char_p,
        ctypes.c_int,
        ctypes.c_char_p,
        ctypes.c_uint,
    )
    renameat2.restype = ctypes.c_int
    ctypes.set_errno(0)
    if renameat2(
        _AT_FDCWD,
        os.fsencode(staged),
        _AT_FDCWD,
        os.fsencode(destination),
        _RENAME_NOREPLACE,
    ):
        _raise_posix_rename_error(staged, destination)


def _macos_rename_exclusive(staged: Path, destination: Path) -> None:
    """Use macOS renamex_np so a competing destination is never replaced."""

    libc = ctypes.CDLL(None, use_errno=True)
    try:
        renamex_np = libc.renamex_np
    except AttributeError as exc:
        raise OSError(
            errno.ENOSYS,
            "libc does not expose renamex_np for exclusive directory publication",
        ) from exc
    renamex_np.argtypes = (ctypes.c_char_p, ctypes.c_char_p, ctypes.c_uint)
    renamex_np.restype = ctypes.c_int
    ctypes.set_errno(0)
    if renamex_np(os.fsencode(staged), os.fsencode(destination), _RENAME_EXCL):
        _raise_posix_rename_error(staged, destination)


def _namespace_publish_directory_exclusive_once(
    staged: Path, destination: Path
) -> None:
    """Atomically rename a directory while refusing every destination collision."""

    if os.name == "nt":
        windows_replace_write_through(
            staged,
            destination,
            replace_once=lambda source, target: move_file_ex_write_through(
                source,
                target,
                replace_existing=False,
            ),
        )
    elif sys.platform.startswith("linux"):
        _linux_rename_exclusive(staged, destination)
    elif sys.platform == "darwin":
        _macos_rename_exclusive(staged, destination)
    else:
        raise OSError(
            errno.ENOTSUP,
            f"exclusive directory publication is unsupported on {sys.platform}",
        )


def _real_directory(path: Path, *, label: str) -> os.stat_result:
    try:
        metadata = path.lstat()
    except OSError as exc:
        raise ValueError(f"{label} is unavailable: {path}: {exc}") from exc
    if not stat.S_ISDIR(metadata.st_mode) or _metadata_is_link_like(metadata):
        raise ValueError(f"{label} is not a real directory: {path}")
    return metadata


def _flush_staged_directory_tree(root: Path) -> None:
    """Flush staged file contents and namespace owners without following links."""

    root = Path(root)
    _real_directory(root, label="staged publication source")
    directories: list[Path] = []
    pending = [root]
    flushed_files: set[tuple[int, int]] = set()
    while pending:
        directory_path = pending.pop()
        _real_directory(directory_path, label="staged publication directory")
        directories.append(directory_path)
        try:
            entries = os.scandir(directory_path)
        except OSError as exc:
            raise ValueError(
                f"staged publication directory cannot be inventoried: "
                f"{directory_path}: {exc}"
            ) from exc
        with entries:
            for entry in entries:
                child = directory_path / entry.name
                try:
                    # Path.lstat obtains stable file indexes on Windows, while
                    # cached DirEntry metadata can report st_ino=0 and defeat
                    # hard-link de-duplication.
                    metadata = child.lstat()
                except OSError as exc:
                    raise ValueError(
                        "staged publication entry cannot be inventoried: "
                        f"{child}: {exc}"
                    ) from exc
                if stat.S_ISLNK(metadata.st_mode):
                    # The namespace entry is made durable by its owning
                    # directory. Never follow a staged symlink to flush bytes
                    # outside the tree.
                    continue
                if _metadata_is_link_like(metadata):
                    raise ValueError(
                        f"staged publication tree contains a reparse point: {child}"
                    )
                if stat.S_ISDIR(metadata.st_mode):
                    pending.append(child)
                    continue
                if not stat.S_ISREG(metadata.st_mode):
                    raise ValueError(
                        f"staged publication tree contains a special entry: {child}"
                    )
                identity = (metadata.st_dev, metadata.st_ino)
                if not metadata.st_ino or identity not in flushed_files:
                    _flush_staged_file(child)
                    if metadata.st_ino:
                        flushed_files.add(identity)
    for directory in reversed(directories):
        fsync_directory(directory)


def _sync_directory_publication_parents_after_commit(
    source_parent: Path, destination_parent: Path
) -> None:
    failures: list[str] = []
    for parent in dict.fromkeys((destination_parent, source_parent)):
        try:
            fsync_directory(parent)
        except OSError as exc:
            failures.append(f"{parent}: {exc}")
    if failures:
        _warn_after_commit(
            "exclusive directory publication committed, but a parent-directory "
            "durability barrier failed: " + "; ".join(failures)
        )


def durable_publish_directory_exclusive(staged: Path, destination: Path) -> None:
    """Atomically publish one staged tree without replacing any destination.

    Every staged regular-file payload and directory namespace crosses its
    durability barrier before the atomic rename. Windows requests a write-through
    rename; Linux and macOS use their native no-replace rename APIs. Once that
    rename succeeds the public name is committed, so a later parent-directory
    fsync failure is reported as a warning rather than as a false rollback.
    """

    staged = Path(staged).absolute()
    destination = Path(destination).absolute()
    if not staged.name or not destination.name:
        raise ValueError("directory publication requires source and destination leaves")
    source_parent = staged.parent.resolve(strict=True)
    destination_parent = destination.parent.resolve(strict=True)
    staged = source_parent / staged.name
    destination = destination_parent / destination.name
    _real_directory(source_parent, label="staged publication parent")
    _real_directory(destination_parent, label="directory publication parent")
    _flush_staged_directory_tree(staged)
    try:
        destination_metadata = destination.lstat()
    except FileNotFoundError:
        pass
    except OSError as exc:
        raise ValueError(
            f"directory publication destination cannot be inventoried: "
            f"{destination}: {exc}"
        ) from exc
    else:
        if _metadata_is_link_like(destination_metadata):
            raise ValueError(
                f"directory publication destination is indirect: {destination}"
            )
        raise FileExistsError(
            errno.EEXIST,
            "directory publication destination already exists",
            destination,
        )
    _namespace_publish_directory_exclusive_once(staged, destination)
    _sync_directory_publication_parents_after_commit(source_parent, destination_parent)


def _flush_staged_file(staged: Path) -> int:
    metadata = staged.lstat()
    if not stat.S_ISREG(metadata.st_mode) or _metadata_is_link_like(metadata):
        raise ValueError(f"staged publication source is not a real file: {staged}")
    staged_was_readonly = not metadata.st_mode & stat.S_IWRITE
    if staged_was_readonly:
        staged.chmod(metadata.st_mode | stat.S_IWRITE)
    try:
        with staged.open("r+b") as handle:
            before = os.fstat(handle.fileno())
            if (before.st_dev, before.st_ino) != (metadata.st_dev, metadata.st_ino):
                raise ValueError(f"staged publication source changed: {staged}")
            os.fsync(handle.fileno())
            after = os.fstat(handle.fileno())
        final = staged.lstat()
    finally:
        if staged_was_readonly and staged.exists() and not is_link_like(staged):
            staged.chmod(metadata.st_mode)
    if (before.st_dev, before.st_ino, before.st_size) != (
        after.st_dev,
        after.st_ino,
        after.st_size,
    ) or (after.st_dev, after.st_ino, after.st_size) != (
        final.st_dev,
        final.st_ino,
        final.st_size,
    ):
        raise ValueError(f"staged publication source changed: {staged}")
    return final.st_size


def durable_replace(staged: Path, destination: Path) -> None:
    """Durably replace one public file with one same-filesystem staged file."""

    staged = Path(staged)
    destination = Path(destination)
    _flush_staged_file(staged)
    if is_link_like(destination):
        raise ValueError(f"file publication destination is indirect: {destination}")
    if destination.exists() and not destination.is_file():
        raise ValueError(
            f"file publication destination is not a regular file: {destination}"
        )
    replaced_mode: int | None = None
    try:
        if destination.exists() and not destination.stat().st_mode & stat.S_IWRITE:
            replaced_mode = destination.stat().st_mode
            destination.chmod(replaced_mode | stat.S_IWRITE)
        durable_namespace_replace(staged, destination)
    except BaseException:
        if replaced_mode is not None and destination.exists():
            with contextlib.suppress(OSError):
                destination.chmod(replaced_mode)
        raise


def durable_publish_exclusive(staged: Path, destination: Path) -> None:
    """Durably publish a staged file without replacing an existing leaf."""

    staged = Path(staged)
    destination = Path(destination)
    if staged.parent != destination.parent:
        raise ValueError("exclusive file publication requires one directory authority")
    _flush_staged_file(staged)
    if destination.exists() or is_link_like(destination):
        raise FileExistsError(destination)
    if os.name == "nt":
        windows_replace_write_through(
            staged,
            destination,
            replace_once=lambda source, target: move_file_ex_write_through(
                source,
                target,
                replace_existing=False,
            ),
        )
    elif os.name == "posix":
        os.link(staged, destination, follow_symlinks=False)
        fsync_directory(destination.parent)
        try:
            staged.unlink()
            fsync_directory(destination.parent)
        except OSError as exc:
            _warn_after_commit(
                f"exclusive file publication retained staged residue {staged}: {exc}"
            )
    else:
        raise OSError(f"unsupported exclusive publication platform: {os.name}")


def _canonical_leaf(path: Path, *, create_parent: bool) -> Path:
    path = Path(path)
    if not path.name:
        raise ValueError(f"file publication requires a leaf path: {path}")
    if create_parent:
        path.parent.mkdir(parents=True, exist_ok=True)
    parent = path.parent.resolve(strict=True)
    leaf = parent / path.name
    if is_link_like(leaf):
        raise ValueError(f"file publication destination is indirect: {leaf}")
    if leaf.exists() and not leaf.is_file():
        raise ValueError(f"file publication destination is not a file: {leaf}")
    return leaf


def staged_file_path(destination: Path, *, purpose: str = "write") -> Path:
    """Return a bounded, destination-bound staging path in the same directory."""

    destination = _canonical_leaf(destination, create_parent=True)
    identity = hashlib.sha256(os.fsencode(destination.name)).hexdigest()[:16]
    return destination.parent / f".molt-{purpose}-{identity}-{uuid.uuid4().hex}.tmp"


def atomic_write_bytes(
    path: Path,
    data: bytes,
    *,
    exclusive: bool = False,
    replace: DurableReplace | None = None,
) -> None:
    """Publish complete bytes atomically after crossing the durability barrier."""

    destination = _canonical_leaf(path, create_parent=True)
    if exclusive and destination.exists():
        raise FileExistsError(destination)
    staged = staged_file_path(destination)
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    flags |= getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(staged, flags, 0o666)
    try:
        with os.fdopen(descriptor, "wb", buffering=0) as stream:
            pending = memoryview(data)
            while pending:
                written = stream.write(pending)
                if written is None or written <= 0:
                    raise OSError("short atomic publication write")
                pending = pending[written:]
        if exclusive:
            durable_publish_exclusive(staged, destination)
        else:
            (replace or durable_replace)(staged, destination)
    finally:
        with contextlib.suppress(OSError):
            staged.unlink()
