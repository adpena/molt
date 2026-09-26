"""Cross-platform crash-consistent publication of one filesystem leaf."""

from __future__ import annotations

from collections.abc import Callable
import contextlib
import ctypes
import errno
import hashlib
import os
import re
from pathlib import Path
import stat
import sys
import time
from typing import Protocol
import uuid
import warnings

from molt.file_deletion import delete_path


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
    """

    lexical = Path(path).expanduser()
    if not lexical.is_absolute():
        lexical = Path.cwd() / lexical
    cursor = Path(lexical.anchor)
    for part in lexical.parts[1:]:
        cursor /= part
        if is_link_like(cursor):
            raise ValueError(f"owned path traverses a link or junction: {cursor}")
    return lexical.resolve()


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


def _namespace_publish_leaf_exclusive_once(staged: Path, destination: Path) -> None:
    """Atomically rename a leaf while refusing every destination collision."""

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
            f"exclusive leaf publication is unsupported on {sys.platform}",
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
                    _flush_staged_file(child)
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


def durable_namespace_publish_directory_exclusive(
    source: Path, destination: Path
) -> None:
    """Move an already-owned tree without following it or replacing a rival.

    This namespace-only operation is for quarantine: damaged contents must be
    preserved even when they cannot pass the staged-payload durability barrier.
    Normal artifact publication must use durable_publish_directory_exclusive.
    """

    source = resolve_owned_path(source)
    destination = resolve_owned_path(destination)
    _real_directory(source, label="quarantine source")
    _real_directory(source.parent, label="quarantine source parent")
    _real_directory(destination.parent, label="quarantine destination parent")
    _namespace_publish_leaf_exclusive_once(source, destination)
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
    return ".molt-retired-v1-" + hashlib.sha256(os.fsencode(scope)).hexdigest() + "-"


def _retirement_identity(metadata: os.stat_result) -> tuple[str, int, int]:
    if metadata_is_link_like(metadata):
        raise ValueError("retirement refuses indirect leaves")
    kind = "d" if stat.S_ISDIR(metadata.st_mode) else "f"
    if kind == "f" and not stat.S_ISREG(metadata.st_mode):
        raise ValueError("retirement refuses special entries")
    if not metadata.st_ino:
        raise ValueError("retirement requires a stable filesystem file identity")
    return kind, metadata.st_dev, metadata.st_ino


def _reclaim_retired_leaf(retired: Path, identity: tuple[str, int, int]) -> None:
    # The caller owns this namespace exclusively. Recheck lexical no-follow
    # custody and identity immediately before reclamation; never adopt a rival
    # that replaced a retired name. rmtree does not follow contained symlinks
    # or Windows directory junctions.
    try:
        resolved = resolve_owned_path(retired)
        if resolved != retired or _retirement_identity(retired.lstat()) != identity:
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
        parent_metadata = parent.lstat()
    except FileNotFoundError:
        return ()
    _real_directory(parent, label="retirement parent")
    parent_identity = _retirement_identity(parent_metadata)
    # Also retries a previous post-removal barrier when no residue remains.
    # More importantly, no physical deletion precedes durable retirement.
    fsync_directory(parent)
    reclaimed: list[Path] = []
    for retired in sorted(parent.iterdir()):
        if not retired.name.startswith(prefix):
            continue
        match = re.fullmatch(
            r"([df])-([0-9a-f]+)-([0-9a-f]+)-([0-9a-f]{16})",
            retired.name[len(prefix) :],
        )
        if match is None:
            raise ValueError(f"malformed retired leaf in owned scope: {retired}")
        if (
            resolve_owned_path(parent) != parent
            or _retirement_identity(parent.lstat()) != parent_identity
        ):
            raise ValueError(f"retirement parent identity changed: {parent}")
        identity = (match[1], int(match[2], 16), int(match[3], 16))
        _reclaim_retired_leaf(retired, identity)
        reclaimed.append(retired)
    return tuple(reclaimed)


def durable_remove_path(path: Path, *, retirement_scope: str | None = None) -> None:
    """Retire one owned live leaf atomically, then reclaim its physical storage.

    The caller retains its existing exclusive namespace lock/private-parent
    custody through this operation and subsequent scoped recovery. No replacement
    of the live name or retired name is permitted. Once the rename commits, every
    error names recoverable retirement state; callers must not recreate a journal
    or claim rollback. A new live leaf at the old spelling is never touched by
    reclaim_retired_paths.
    """
    path = resolve_owned_path(path)
    if path == path.parent or is_link_like(path):
        raise ValueError(f"cleanup requires a real owned leaf: {path}")
    scope = path.name if retirement_scope is None else retirement_scope
    prefix = _retirement_prefix(scope)
    reclaim_retired_paths(path.parent, retirement_scope=scope)
    try:
        metadata = path.lstat()
    except FileNotFoundError:
        return
    identity = _retirement_identity(metadata)
    parent_identity = _retirement_identity(
        _real_directory(path.parent, label="retirement parent")
    )
    leaf_digest = hashlib.sha256(os.fsencode(path.name)).hexdigest()[:16]
    kind, device, inode = identity
    retired = path.parent / f"{prefix}{kind}-{device:x}-{inode:x}-{leaf_digest}"
    if (
        resolve_owned_path(path) != path
        or _retirement_identity(path.lstat()) != identity
        or _retirement_identity(path.parent.lstat()) != parent_identity
    ):
        raise ValueError(f"retirement source identity changed: {path}")
    # Same-parent no-replace rename serves BOTH files and directories. Unlike
    # publication this must not flush/interpret damaged transaction contents.
    # Windows uses MoveFileExW(WRITE_THROUGH), POSIX its no-replace rename.
    _namespace_publish_leaf_exclusive_once(path, retired)
    try:
        if (
            resolve_owned_path(retired) != retired
            or _retirement_identity(retired.lstat()) != identity
            or _retirement_identity(path.parent.lstat()) != parent_identity
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


def _flush_staged_file(staged: Path) -> int:
    metadata = staged.lstat()
    if not stat.S_ISREG(metadata.st_mode) or metadata_is_link_like(metadata):
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
        _sync_publication_parents_after_commit(destination.parent, destination.parent)
        try:
            staged.unlink()
            _sync_publication_parents_after_commit(staged.parent, staged.parent)
        except OSError as exc:
            _warn_after_commit(
                f"exclusive file publication retained staged residue {staged}: {exc}"
            )
    else:
        raise OSError(f"unsupported exclusive publication platform: {os.name}")


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
    identity = hashlib.sha256(os.fsencode(destination.name)).hexdigest()[:16]
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
    staged = Path(staged)
    destination = Path(destination)
    match = _STAGED_NAME_RE.fullmatch(staged.name)
    if match is None or staged.parent != destination.parent:
        return False
    identity = hashlib.sha256(os.fsencode(destination.name)).hexdigest()[:16]
    return (
        match.group("identity") == identity
        and (purpose is None or match.group("purpose") == _staged_purpose(purpose))
        and (suffix is None or match.group("suffix") == suffix)
    )


def is_staged_file_path(path: Path) -> bool:
    """Recognize reserved private staging names, without claiming ownership."""
    return _STAGED_NAME_RE.fullmatch(path.name.casefold()) is not None


def atomic_write_bytes(
    path: Path,
    data: bytes,
    *,
    exclusive: bool = False,
    replace: DurableReplace | None = None,
) -> None:
    """Publish complete bytes atomically after crossing the durability barrier."""

    destination = canonical_file_leaf(path, create_parent=True)
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
