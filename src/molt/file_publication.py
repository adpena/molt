"""Cross-platform atomic publication with explicit durability for each owner."""

from __future__ import annotations

from collections.abc import Callable
import contextlib
import ctypes
import errno
from enum import Enum
import hashlib
import os
import re
from pathlib import Path
import shutil
import stat
import sys
import time
from typing import Protocol
import uuid
import warnings

from molt.file_deletion import delete_path
from molt.durable_file_identity import file_identity, upgrade_legacy


MOVEFILE_REPLACE_EXISTING = 0x1
MOVEFILE_WRITE_THROUGH = 0x8
WINDOWS_REPLACE_RETRY_ERRORS = frozenset({5, 32, 33})
_AT_FDCWD = -100
_FICLONE = 0x40049409
# A volume or pair of volumes that cannot share blocks answers with one of
# these; the fallback copies through the already-owned file descriptors.
_CLONE_UNSUPPORTED_ERRNOS = frozenset(
    {errno.ENOTSUP, errno.EOPNOTSUPP, errno.EXDEV, errno.EINVAL, errno.ENOTTY}
)
_RENAME_NOREPLACE = 0x1
_RENAME_EXCL = 0x4
_WINDOWS_REPARSE_POINT = getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0)


class MoveFileEx(Protocol):
    def __call__(self, source: str, destination: str, flags: int, /) -> int: ...


GetLastError = Callable[[], int]
FileReplace = Callable[[Path, Path], None]


class PublicationDurability(Enum):
    """Outputs request durability barriers; disposable caches may be rebuilt."""

    DURABLE = "durable"
    RECONSTRUCTIBLE = "reconstructible"


def _require_durability(durability: PublicationDurability) -> None:
    if not isinstance(durability, PublicationDurability):
        raise ValueError(f"invalid publication durability: {durability!r}")


def _warn_after_commit(message: str) -> None:
    try:
        with warnings.catch_warnings():
            warnings.simplefilter("always", RuntimeWarning)
            warnings.warn(message, RuntimeWarning, stacklevel=3)
    except BaseException:
        pass


def metadata_is_link_like(metadata: os.stat_result) -> bool:
    """Classify already-read no-follow metadata without another filesystem query."""
    return stat.S_ISLNK(metadata.st_mode) or bool(
        getattr(metadata, "st_file_attributes", 0) & _WINDOWS_REPARSE_POINT
    )


def is_link_like(path: Path) -> bool:
    """Return whether a path is a symlink or any Windows reparse point."""

    try:
        metadata = path.lstat()
    except (FileNotFoundError, NotADirectoryError):
        return False
    return metadata_is_link_like(metadata)


def resolve_owned_path(path: Path) -> Path:
    """Resolve custody only after rejecting each lexical link/reparse component.

    Check the supplied spelling before resolution can erase an indirect root,
    including dangling links and components preceding a parent traversal.
    Windows DOS and UNC paths have one spelling, independent of whether the
    caller or the filesystem resolver supplied an extended-length prefix.
    """

    lexical = Path(path).expanduser()
    if not lexical.is_absolute():
        lexical = Path.cwd() / lexical
    cursor = Path(lexical.anchor)
    for part in lexical.parts[1:]:
        cursor /= part
        if is_link_like(cursor):
            raise ValueError(f"owned path traverses a link or junction: {cursor}")
    resolved = lexical.resolve()
    if os.name == "nt":
        spelling = str(resolved)
        if spelling.startswith("\\\\?\\UNC\\"):
            resolved = Path("\\\\" + spelling[8:])
        elif spelling.startswith("\\\\?\\") and re.match(r"[a-zA-Z]:\\", spelling[4:]):
            resolved = Path(spelling[4:])
    return resolved


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
    replace_once: FileReplace = move_file_ex_write_through,
) -> None:
    _windows_replace(staged, destination, replace_once=replace_once)


def _windows_replace(
    staged: Path, destination: Path, *, replace_once: FileReplace
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


def namespace_replace_once(
    staged: Path,
    destination: Path,
    *,
    durability: PublicationDurability = PublicationDurability.DURABLE,
) -> None:
    _require_durability(durability)
    if os.name == "nt":
        if durability is PublicationDurability.DURABLE:
            windows_replace_write_through(staged, destination)
        else:
            # CPython's same-filesystem os.replace uses REPLACE_EXISTING,
            # without WRITE_THROUGH. Sharing failures retain the old leaf.
            _windows_replace(staged, destination, replace_once=os.replace)
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
    _sync_publication_parents_after_commit(source_parent, destination_parent)


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
            "libc does not expose renameat2 for exclusive leaf publication",
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
            "libc does not expose renamex_np for exclusive leaf publication",
        ) from exc
    renamex_np.argtypes = (ctypes.c_char_p, ctypes.c_char_p, ctypes.c_uint)
    renamex_np.restype = ctypes.c_int
    ctypes.set_errno(0)
    if renamex_np(os.fsencode(staged), os.fsencode(destination), _RENAME_EXCL):
        _raise_posix_rename_error(staged, destination)


def _namespace_publish_leaf_exclusive_once(
    staged: Path,
    destination: Path,
    *,
    durability: PublicationDurability = PublicationDurability.DURABLE,
) -> None:
    """Atomically rename a leaf while refusing every destination collision.

    Only a durable rename requests Windows write-through. CPython's
    ``os.rename`` is MoveFileExW without REPLACE_EXISTING, so it refuses a
    rival without a barrier; POSIX has no barrier in the rename itself.
    """

    if os.name == "nt":
        if durability is PublicationDurability.DURABLE:
            windows_replace_write_through(
                staged,
                destination,
                replace_once=lambda source, target: move_file_ex_write_through(
                    source,
                    target,
                    replace_existing=False,
                ),
            )
        else:
            _windows_replace(staged, destination, replace_once=os.rename)
    elif sys.platform.startswith("linux"):
        _linux_rename_exclusive(staged, destination)
    elif sys.platform == "darwin":
        _macos_rename_exclusive(staged, destination)
    else:
        raise OSError(
            errno.ENOTSUP,
            f"exclusive leaf publication is unsupported on {sys.platform}",
        )


def namespace_move_exclusive(source: Path, destination: Path) -> None:
    """Move one leaf atomically, refusing a rival; no durability barrier.

    Only for records whose loss on a crash is harmless: the move is atomic,
    but power loss can roll it back, and the owner's next pass repeats it.
    Everything else uses the durable publication functions.
    """
    _namespace_publish_leaf_exclusive_once(
        Path(source),
        Path(destination),
        durability=PublicationDurability.RECONSTRUCTIBLE,
    )


def _real_directory(path: Path, *, label: str) -> os.stat_result:
    try:
        metadata = path.lstat()
    except OSError as exc:
        raise ValueError(f"{label} is unavailable: {path}: {exc}") from exc
    if not stat.S_ISDIR(metadata.st_mode) or metadata_is_link_like(metadata):
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
                if metadata_is_link_like(metadata):
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
                    _validate_staged_file(
                        child, durability=PublicationDurability.DURABLE
                    )
                    if metadata.st_ino:
                        flushed_files.add(identity)
    for directory in reversed(directories):
        fsync_directory(directory)


def _sync_publication_parents_after_commit(
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
            "namespace publication committed, but a parent-directory "
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
        if metadata_is_link_like(destination_metadata):
            raise ValueError(
                f"directory publication destination is indirect: {destination}"
            )
        raise FileExistsError(
            errno.EEXIST,
            "directory publication destination already exists",
            destination,
        )
    _namespace_publish_leaf_exclusive_once(staged, destination)
    _sync_publication_parents_after_commit(source_parent, destination_parent)


def namespace_publish_directory_exclusive(
    source: Path,
    destination: Path,
    *,
    durability: PublicationDurability = PublicationDurability.DURABLE,
) -> None:
    """Move an already-owned tree without following it or replacing a rival.

    This namespace-only operation moves a tree whose contents it must not
    interpret: quarantine preserves damaged contents that cannot pass the
    staged-payload durability barrier, and guard scratch retires a payload.
    A durable move then flushes both parent directories. A reconstructible
    move keeps every custody check but adds no barrier: an OS crash can roll
    it back, so its owner's records must name both outcomes. Normal artifact
    publication must use durable_publish_directory_exclusive.
    """

    _require_durability(durability)
    source = resolve_owned_path(source)
    destination = resolve_owned_path(destination)
    _real_directory(source, label="namespace move source")
    _real_directory(source.parent, label="namespace move source parent")
    _real_directory(destination.parent, label="namespace move destination parent")
    _namespace_publish_leaf_exclusive_once(source, destination, durability=durability)
    if durability is PublicationDurability.DURABLE:
        _sync_publication_parents_after_commit(source.parent, destination.parent)


class RetirementError(OSError):
    """The live name is retired; only physical reclamation/durability failed."""

    namespace_committed = True

    def __init__(self, retired_path: Path, phase: str, cause: BaseException) -> None:
        self.retired_path = retired_path
        self.source_path: Path | None = None
        self.phase = phase
        super().__init__(
            f"namespace retirement committed; {phase} failed; "
            f"recoverable retired leaf {retired_path}: {cause}"
        )


def _retirement_prefix(scope: str) -> str:
    if not isinstance(scope, str) or not scope:
        raise ValueError("retirement requires a nonempty caller-owned scope")
    return ".molt-retired-v2-" + hashlib.sha256(os.fsencode(scope)).hexdigest() + "-"


def _retirement_identity(path: Path) -> tuple[str, str]:
    metadata = path.lstat()
    if metadata_is_link_like(metadata):
        raise ValueError("retirement refuses indirect leaves")
    kind = "d" if stat.S_ISDIR(metadata.st_mode) else "f"
    if kind == "f" and not stat.S_ISREG(metadata.st_mode):
        raise ValueError("retirement refuses special entries")
    identity = file_identity(path, expected=metadata)
    # A digest keeps even long filesystem UUIDs inside the component budget.
    import json

    digest = hashlib.sha256(
        json.dumps(identity.payload(), sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()
    return kind, digest


def _reclaim_retired_leaf(retired: Path, identity: tuple[str, str]) -> None:
    # The caller owns this namespace exclusively. Recheck lexical no-follow
    # custody and identity immediately before reclamation; never adopt a rival
    # that replaced a retired name. rmtree does not follow contained symlinks
    # or Windows directory junctions.
    try:
        resolved = resolve_owned_path(retired)
        if resolved != retired or _retirement_identity(retired) != identity:
            raise ValueError(f"retired leaf identity changed: {retired}")
        removed, error = delete_path(retired)
        if not removed:
            raise OSError(error)
    except (OSError, ValueError) as exc:
        raise RetirementError(retired, "physical reclamation", exc) from exc
    try:
        fsync_directory(retired.parent)
    except OSError as exc:
        raise RetirementError(retired, "reclamation durability", exc) from exc


def reclaim_retired_paths(parent: Path, *, retirement_scope: str) -> tuple[Path, ...]:
    """Reclaim only identity-bound retired leaves in one exclusively owned scope.

    Call under the existing consumer lock, or with a private transaction parent.
    This never retires a live path. Scope hashes and root identities are encoded
    in bounded names, not in a second journal that reclamation could destroy.
    An unknown/malformed/changed retired leaf fails closed and remains evidence.
    """
    prefix = _retirement_prefix(retirement_scope)
    parent = resolve_owned_path(parent)
    try:
        parent.lstat()
    except FileNotFoundError:
        return ()
    _real_directory(parent, label="retirement parent")
    parent_identity = _retirement_identity(parent)
    # Also retries a previous post-removal barrier when no residue remains.
    # More importantly, no physical deletion precedes durable retirement.
    fsync_directory(parent)
    reclaimed: list[Path] = []
    legacy_prefix = prefix.replace(".molt-retired-v2-", ".molt-retired-v1-", 1)
    for retired in sorted(parent.iterdir()):
        if retired.name.startswith(legacy_prefix):
            match = re.fullmatch(
                r"([df])-([0-9a-f]+)-([0-9a-f]+)-([0-9a-f]{16})",
                retired.name[len(legacy_prefix) :],
            )
            if match is None:
                raise ValueError(f"malformed legacy retired leaf: {retired}")
            upgrade_legacy(
                retired, {"device": int(match[2], 16), "inode": int(match[3], 16)}
            )
            kind, digest = _retirement_identity(retired)
            if kind != match[1]:
                raise ValueError("legacy retired leaf kind changed")
            upgraded = parent / f"{prefix}{kind}-{digest}-{match[4]}"
            _namespace_publish_leaf_exclusive_once(retired, upgraded)
            fsync_directory(parent)
            retired = upgraded
        if not retired.name.startswith(prefix):
            continue
        match = re.fullmatch(
            r"([df])-([0-9a-f]{64})-([0-9a-f]{16})",
            retired.name[len(prefix) :],
        )
        if match is None:
            raise ValueError(f"malformed retired leaf in owned scope: {retired}")
        if (
            resolve_owned_path(parent) != parent
            or _retirement_identity(parent) != parent_identity
        ):
            raise ValueError(f"retirement parent identity changed: {parent}")
        identity = (match[1], match[2])
        _reclaim_retired_leaf(retired, identity)
        reclaimed.append(retired)
    return tuple(reclaimed)


def durable_remove_path(
    path: Path,
    *,
    retirement_scope: str | None = None,
    reclaim_existing: bool = True,
) -> None:
    """Retire one owned live leaf atomically, then reclaim its physical storage.

    The caller retains its existing exclusive namespace lock/private-parent
    custody through this operation and subsequent scoped recovery. No replacement
    of the live name or retired name is permitted. Once the rename commits, every
    error names recoverable retirement state; callers must not recreate a journal
    or claim rollback. A new live leaf at the old spelling is never touched by
    reclaim_retired_paths.

    A batch owner may set ``reclaim_existing=False`` after its one scoped
    recovery sweep under the same lock. This avoids rescanning the shared
    parent for every entry; retirement and failure recovery remain identical.
    """
    path = resolve_owned_path(path)
    if path == path.parent or is_link_like(path):
        raise ValueError(f"cleanup requires a real owned leaf: {path}")
    scope = path.name if retirement_scope is None else retirement_scope
    prefix = _retirement_prefix(scope)
    if reclaim_existing:
        reclaim_retired_paths(path.parent, retirement_scope=scope)
    try:
        path.lstat()
    except FileNotFoundError:
        return
    identity = _retirement_identity(path)
    parent_identity = _retirement_identity(path.parent)
    leaf_digest = hashlib.sha256(os.fsencode(path.name)).hexdigest()[:16]
    kind, digest = identity
    retired = path.parent / f"{prefix}{kind}-{digest}-{leaf_digest}"
    if (
        resolve_owned_path(path) != path
        or _retirement_identity(path) != identity
        or _retirement_identity(path.parent) != parent_identity
    ):
        raise ValueError(f"retirement source identity changed: {path}")
    # Same-parent no-replace rename serves BOTH files and directories. Unlike
    # publication this must not flush/interpret damaged transaction contents.
    # Windows uses MoveFileExW(WRITE_THROUGH), POSIX its no-replace rename.
    _namespace_publish_leaf_exclusive_once(path, retired)
    try:
        if (
            resolve_owned_path(retired) != retired
            or _retirement_identity(retired) != identity
            or _retirement_identity(path.parent) != parent_identity
        ):
            raise ValueError(f"retirement namespace identity changed: {retired}")
        fsync_directory(path.parent)
    except (OSError, ValueError) as exc:
        error = RetirementError(retired, "retirement durability/identity", exc)
        error.source_path = path
        raise error from exc
    try:
        _reclaim_retired_leaf(retired, identity)
    except RetirementError as exc:
        exc.source_path = path
        raise


def _validate_staged_file(staged: Path, *, durability: PublicationDurability) -> int:
    metadata = staged.lstat()
    if not stat.S_ISREG(metadata.st_mode) or metadata_is_link_like(metadata):
        raise ValueError(f"staged publication source is not a real file: {staged}")
    flush = durability is PublicationDurability.DURABLE
    restore_readonly = flush and not metadata.st_mode & stat.S_IWRITE
    if restore_readonly:
        staged.chmod(metadata.st_mode | stat.S_IWRITE)
    try:
        with staged.open("r+b" if flush else "rb") as handle:
            before = os.fstat(handle.fileno())
            if (before.st_dev, before.st_ino) != (metadata.st_dev, metadata.st_ino):
                raise ValueError(f"staged publication source changed: {staged}")
            if flush:
                os.fsync(handle.fileno())
            after = os.fstat(handle.fileno())
        final = staged.lstat()
    finally:
        if restore_readonly and staged.exists() and not is_link_like(staged):
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

    atomic_replace(staged, destination)


def atomic_replace(
    staged: Path,
    destination: Path,
    *,
    durability: PublicationDurability = PublicationDurability.DURABLE,
) -> None:
    """Replace one complete staged file with the owner's durability contract.

    Reconstructible caches retain atomic visibility and custody checks but need
    no file or namespace durability barrier. Their consumers must independently
    qualify cached data and reconstruct a missing or corrupt entry.
    """

    _require_durability(durability)
    staged = Path(staged)
    destination = Path(destination)
    _validate_staged_file(staged, durability=durability)
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
        if durability is PublicationDurability.DURABLE:
            durable_namespace_replace(staged, destination)
        else:
            namespace_replace_once(staged, destination, durability=durability)
    except BaseException:
        if replaced_mode is not None and destination.exists():
            with contextlib.suppress(OSError):
                destination.chmod(replaced_mode)
        raise


def durable_publish_exclusive(staged: Path, destination: Path) -> None:
    """Publish one stable file generation without replacing an existing leaf."""

    _publish_exclusive(staged, destination, durability=PublicationDurability.DURABLE)


def _publish_exclusive(
    staged: Path, destination: Path, *, durability: PublicationDurability
) -> None:
    """Publish one staged file; never replace a leaf that already exists.

    Exclusivity is a property of the no-replace rename, not of a flush. A
    reconstructible publication keeps the staged-file and destination custody
    checks and skips both barriers: an OS crash may then lose the leaf or
    expose it empty, so its owner must treat both like an absent record.
    """

    staged = Path(staged)
    destination = Path(destination)
    _validate_staged_file(staged, durability=durability)
    if destination.exists() or is_link_like(destination):
        raise FileExistsError(destination)
    # Link-then-unlink changes the visible inode's ctime after publication and
    # invalidates identities already captured by concurrent readers. Use the
    # same exclusive rename authority as directory/leaf publication.
    _namespace_publish_leaf_exclusive_once(staged, destination, durability=durability)
    if durability is PublicationDurability.DURABLE:
        _sync_publication_parents_after_commit(staged.parent, destination.parent)


def canonical_file_leaf(
    path: Path,
    *,
    create_parent: bool,
    role: str = "file publication destination",
) -> Path:
    """Canonicalize parent aliases and reject indirect or non-file leaves."""

    path = Path(path)
    if not path.name:
        raise ValueError(f"{role} requires a leaf path: {path}")
    if create_parent:
        path.parent.mkdir(parents=True, exist_ok=True)
    parent = path.parent.resolve(strict=True)
    leaf = parent / path.name
    if is_link_like(leaf):
        raise ValueError(f"{role} is indirect: {leaf}")
    if leaf.exists() and not leaf.is_file():
        raise ValueError(f"{role} is not a file: {leaf}")
    return leaf


_STAGED_NAME_RE = re.compile(
    r"^\.molt-(?P<purpose>[a-z0-9][a-z0-9-]{0,15})-"
    r"(?P<identity>[0-9a-f]{16})-(?P<nonce>[0-9a-f]{32})"
    r"(?P<suffix>\.[^/\\]+)$"
)


def _staged_purpose(purpose: str) -> str:
    normalized = re.sub(r"[^a-z0-9-]+", "-", purpose.strip().casefold()).strip("-")
    return (normalized or "stage")[:16]


def staging_destination_key(destination: Path) -> str:
    """A stage-name key, not physical identity or permission to remove a file."""
    return hashlib.sha256(os.fsencode(destination.name)).hexdigest()[:16]


def staged_file_destination_key(
    staged: Path, *, purpose: str | None = None, suffix: str | None = None
) -> str | None:
    """Read the canonical name key for bounded, owner-scoped stage discovery.

    Consumers must separately prove directory, physical generation, and writer
    custody. In particular a reserved-looking name never authorizes deletion.
    """
    match = _STAGED_NAME_RE.fullmatch(os.path.normcase(staged.name))
    if (
        match is None
        or (purpose is not None and match.group("purpose") != _staged_purpose(purpose))
        or (suffix is not None and match.group("suffix") != os.path.normcase(suffix))
    ):
        return None
    return match.group("identity")


def unlink_owned_file(path: Path, identity: os.stat_result | None) -> None:
    """Reclaim a partial file only while the leaf still names the acquired node.

    Callers own the parent namespace; this identity fence refuses replacements
    observed before unlink, not hostile concurrent mutation of that namespace.
    """

    if identity is None or not identity.st_ino:
        return
    try:
        current = path.lstat()
    except FileNotFoundError:
        return
    if not metadata_is_link_like(current) and os.path.samestat(identity, current):
        path.unlink()


def clone_file(source: Path, destination: Path) -> None:
    """Copy an opened regular source into a missing, independently owned leaf.

    APFS fclonefileat and Linux FICLONE share blocks without traversing bytes.
    Unsupported volumes use bounded copying through the same held descriptors;
    no path is reopened for writing. Source symlinks are followed, but a
    directory or special source is refused before destination acquisition.
    The caller owns the destination parent namespace. On a partial-copy error,
    cleanup checks the acquired node's identity and preserves any replacement.
    """

    # Nonblocking open lets fstat reject FIFOs without waiting for a producer.
    source_fd = os.open(
        source, os.O_RDONLY | getattr(os, "O_NONBLOCK", 0) | getattr(os, "O_BINARY", 0)
    )
    try:
        source_metadata = os.fstat(source_fd)
        if not stat.S_ISREG(source_metadata.st_mode):
            raise OSError(errno.EINVAL, "clone source is not a regular file", source)
        reader = os.fdopen(source_fd, "rb")
    except BaseException:
        os.close(source_fd)
        raise
    with reader:
        if sys.platform == "darwin":
            libc = ctypes.CDLL(None, use_errno=True)
            fclonefileat = libc.fclonefileat
            fclonefileat.argtypes = (
                ctypes.c_int,
                ctypes.c_int,
                ctypes.c_char_p,
                ctypes.c_uint32,
            )
            fclonefileat.restype = ctypes.c_int
            parent_fd = os.open(destination.parent, os.O_RDONLY | os.O_DIRECTORY)
            try:
                ctypes.set_errno(0)
                if not fclonefileat(
                    reader.fileno(), parent_fd, os.fsencode(destination.name), 0
                ):
                    return
                error_code = ctypes.get_errno()
            finally:
                os.close(parent_fd)
            if error_code not in _CLONE_UNSUPPORTED_ERRNOS:
                raise OSError(
                    error_code,
                    os.strerror(error_code),
                    os.fspath(source),
                    os.fspath(destination),
                )
        # fclonefileat is atomic on failure. Acquisition sits outside cleanup:
        # a collision never grants permission to reclaim the existing leaf.
        writer = open(destination, "xb")
        identity: os.stat_result | None = None
        try:
            with writer:
                identity = os.fstat(writer.fileno())
                cloned = False
                if sys.platform.startswith("linux"):
                    import fcntl

                    try:
                        fcntl.ioctl(writer.fileno(), _FICLONE, reader.fileno())
                        cloned = True
                    except OSError as exc:
                        if exc.errno not in _CLONE_UNSUPPORTED_ERRNOS:
                            raise
                if not cloned:
                    shutil.copyfileobj(reader, writer, length=1024 * 1024)
            current = destination.lstat()
            if metadata_is_link_like(current) or not os.path.samestat(
                identity, current
            ):
                raise ValueError(
                    f"clone destination changed during copy: {destination}"
                )
        except BaseException as primary:
            try:
                unlink_owned_file(destination, identity)
            except BaseException as cleanup_error:
                BaseException.add_note(
                    primary,
                    f"clone destination cleanup failed for {destination}: "
                    f"{type(cleanup_error).__name__}: {cleanup_error}",
                )
            raise


def staged_file_path(
    destination: Path, *, purpose: str = "write", suffix: str = ".tmp"
) -> Path:
    """Return a bounded, destination-bound staging path in the same directory.

    Every file-publication consumer uses this naming authority, including when
    its destination is another private stage. Hashing the leaf instead of
    appending to it keeps nested verified copies within filesystem component
    limits. The purpose is a short, fixed call-site label, never a source name.
    This chooses an unreserved name; callers retain their creation/publication
    protocol and must clean up their own stage on failure.
    """

    if (
        not suffix.startswith(".")
        or suffix in {".", ".."}
        or "/" in suffix
        or "\\" in suffix
    ):
        raise ValueError(f"invalid staged file suffix: {suffix!r}")
    destination = canonical_file_leaf(destination, create_parent=True)
    identity = staging_destination_key(destination)
    return destination.parent / (
        f".molt-{_staged_purpose(purpose)}-{identity}-{uuid.uuid4().hex}{suffix}"
    )


def is_owned_staged_file_path(
    staged: Path,
    destination: Path,
    *,
    purpose: str | None = None,
    suffix: str | None = None,
) -> bool:
    """Recognize only a same-directory stage issued by the naming authority."""
    from molt.path_custody import resolved_host_path_key

    staged = Path(staged)
    destination = Path(destination)
    key = staged_file_destination_key(staged, purpose=purpose, suffix=suffix)
    return (
        key is not None
        and key == staging_destination_key(destination)
        and resolved_host_path_key(staged.parent)
        == resolved_host_path_key(destination.parent)
    )


def is_staged_file_path(path: Path) -> bool:
    """Recognize reserved private staging names, without claiming ownership."""
    return _STAGED_NAME_RE.fullmatch(path.name.casefold()) is not None


def atomic_write_bytes(
    path: Path,
    data: bytes,
    *,
    exclusive: bool = False,
    replace: FileReplace | None = None,
    durability: PublicationDurability = PublicationDurability.DURABLE,
    purpose: str = "write",
    create_parent: bool = True,
) -> None:
    """Publish complete bytes atomically with the owner's durability policy.

    ``exclusive`` refuses an existing leaf with a no-replace rename under
    either durability. Only an absent stage is harmless cleanup. A cleanup
    failure propagates, or annotates an existing publication error without
    replacing that error. Without ``create_parent`` an absent parent raises
    ``FileNotFoundError``: a write beside an immutable generation must not
    recreate one that eviction removed.
    """

    _require_durability(durability)
    if durability is PublicationDurability.RECONSTRUCTIBLE and replace is not None:
        raise ValueError("a custom replacement must be durable")
    destination = canonical_file_leaf(path, create_parent=create_parent)
    if exclusive and destination.exists():
        raise FileExistsError(destination)
    staged = staged_file_path(destination, purpose=purpose)
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    flags |= getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(staged, flags, 0o666)
    primary: BaseException | None = None
    try:
        with os.fdopen(descriptor, "wb", buffering=0) as stream:
            pending = memoryview(data)
            while pending:
                written = stream.write(pending)
                if written is None or written <= 0:
                    raise OSError("short atomic publication write")
                pending = pending[written:]
        if exclusive:
            _publish_exclusive(staged, destination, durability=durability)
        elif durability is PublicationDurability.DURABLE:
            (replace or durable_replace)(staged, destination)
        else:
            atomic_replace(staged, destination, durability=durability)
    except BaseException as exc:
        primary = exc
        raise
    finally:
        try:
            staged.unlink()
        except FileNotFoundError:
            pass
        except BaseException as cleanup_error:
            if primary is None:
                raise
            BaseException.add_note(
                primary,
                f"atomic publication stage cleanup failed for {staged}: "
                f"{type(cleanup_error).__name__}: {cleanup_error}",
            )
