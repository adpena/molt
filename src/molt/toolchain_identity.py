"""Exact executable identity shared by build, release, and runtime toolchains."""

from __future__ import annotations

from collections.abc import Generator, Iterator, Mapping, Sequence
from contextlib import closing, contextmanager
from dataclasses import dataclass
import hashlib
from io import BufferedReader
import os
from pathlib import Path
import re
import shlex
import sqlite3
import stat
import subprocess
import time
from typing import BinaryIO

from molt.default_paths import executable_environment_value, expand_user_path
from molt.process_environment import ProcessEnvironment

from molt.file_hashing import (
    DIGEST_RACY_WINDOW_NS,
    content_change_time_ns,
    content_change_time_ns_from_fd,
    open_stable_read_descriptor,
)
from molt.file_publication import metadata_is_link_like, unlink_owned_file
from molt.llvm_linker_roles import lexical_executable_path


@dataclass(frozen=True, slots=True)
class ExecutableIdentity:
    """One invoked entrypoint and the immutable bytes it resolves to."""

    path: Path
    content_path: Path
    size: int
    sha256: str
    version: str

    def content_record(self) -> dict[str, str | int]:
        return {
            "entrypoint": _portable_filename(self.path.name),
            "content_filename": _portable_filename(self.content_path.name),
            "size": self.size,
            "sha256": self.sha256,
        }

    def as_record(self) -> dict[str, str | int]:
        return {
            **self.content_record(),
            "version": self.version,
        }


@dataclass(frozen=True, slots=True)
class StableRegularFileVersion:
    """Metadata observation for one direct regular file; no payload hashing.

    An equal token cannot prove no writes occurred while its handle was closed.
    Windows in particular can assign the same ChangeTime to distinct writes.
    """

    path: Path
    size: int
    _stat_identity: tuple[int, int, int, int, int, int]
    _content_change_time_ns: int

    def stat_token(self) -> tuple[int, int, int, int, int, int]:
        """This observation as a build-record token: (device, inode, mode,
        size, mtime, content-change time), as `molt.input_report` takes it."""
        return (*self._stat_identity[:5], self._content_change_time_ns)


@dataclass(frozen=True, slots=True)
class StableRegularFileIdentity(StableRegularFileVersion):
    """Content and mutation identity for one direct regular file."""

    sha256: str


@dataclass(frozen=True, slots=True)
class StableRegularFileSnapshot:
    """One stable source read streamed into one attested direct snapshot."""

    source: StableRegularFileIdentity
    snapshot: StableRegularFileIdentity
    prefix: bytes

    def discard(self) -> None:
        """Remove this transaction-owned snapshot without deleting a replacement."""

        _unlink_stable_file_identity(self.snapshot)


class StableRegularFileError(ValueError):
    """A direct regular file could not establish stable identity."""


class StableRegularFileChangedError(StableRegularFileError):
    """A direct regular file changed during an identity transaction."""


class StableRegularFileSnapshotError(StableRegularFileError):
    """A transaction-owned snapshot changed or failed independent attestation."""


def _portable_filename(value: str) -> str:
    """Canonicalize case-insensitive Windows entrypoint names on the wire."""

    return value.casefold() if os.name == "nt" else value


_StatToken = tuple[int, int, int, int, int, int]


def _stat_identity(value: os.stat_result) -> _StatToken:
    return (
        value.st_dev,
        value.st_ino,
        value.st_mode,
        value.st_size,
        value.st_mtime_ns,
        value.st_ctime_ns,
    )


def _path_handle_identity(
    value: os.stat_result,
) -> tuple[int, int, int, int, int]:
    """Return fields represented identically by path and handle APIs."""

    return (
        value.st_dev,
        value.st_ino,
        stat.S_IFMT(value.st_mode),
        value.st_size,
        value.st_mtime_ns,
    )


def _stored_path_handle_identity(
    value: tuple[int, int, int, int, int, int],
) -> tuple[int, int, int, int, int]:
    """Project a stored full stat identity onto path/handle-stable fields."""

    device, inode, mode, size, mtime_ns, _ctime_ns = value
    return device, inode, stat.S_IFMT(mode), size, mtime_ns


def stable_regular_file_path_is_current(
    identity: StableRegularFileVersion, metadata: os.stat_result
) -> bool:
    """Whether no-follow path metadata still names the captured generation.

    This path-level fence compares object, type, length, mtime and content
    change time without reopening the payload. Handle-level custody remains
    ``verify_stable_regular_file_identity``.
    """

    return (
        stat.S_ISREG(metadata.st_mode)
        and not metadata_is_link_like(metadata)
        and _path_handle_identity(metadata)
        == _stored_path_handle_identity(identity._stat_identity)
        and content_change_time_ns(identity.path, metadata)
        == identity._content_change_time_ns
    )


def _unlink_stable_file_identity(identity: StableRegularFileIdentity) -> None:
    """Unlink only a path that still names the attested file generation."""

    try:
        current = identity.path.lstat()
    except FileNotFoundError:
        return
    if stable_regular_file_path_is_current(identity, current):
        identity.path.unlink()


def _executable_paths(path: Path, *, label: str) -> tuple[Path, Path]:
    """Return the lexical entrypoint and its resolved regular-file content."""

    lexical = lexical_executable_path(path)
    try:
        metadata = lexical.lstat()
    except OSError as exc:
        raise ValueError(f"{label} is unavailable: {lexical}") from exc
    if not (stat.S_ISREG(metadata.st_mode) or stat.S_ISLNK(metadata.st_mode)):
        raise ValueError(f"{label} is not a file entrypoint: {lexical}")
    try:
        resolved = lexical.resolve(strict=True)
    except OSError as exc:
        raise ValueError(f"{label} cannot be resolved: {lexical}") from exc
    if not resolved.is_file() or resolved.is_symlink() or resolved.is_junction():
        raise ValueError(f"{label} does not resolve to a real file: {resolved}")
    return lexical, resolved


def executable_content_path(path: Path, *, label: str) -> Path:
    return _executable_paths(path, label=label)[1]


def executable_name_candidates(
    command: str, *, environment: Mapping[str, str], windows: bool | None = None
) -> tuple[str, ...]:
    """Expand executable suffixes solely from the captured PATHEXT policy."""
    if windows is None:
        windows = os.name == "nt"
    environment = ProcessEnvironment(environment)
    if not windows:
        return (command,)
    extensions = tuple(
        extension.rstrip(".")
        for extension in (
            executable_environment_value(environment, "PATHEXT")
            or ".COM;.EXE;.BAT;.CMD;.VBS;.JS;.WS;.MSC"
        ).split(";")
        if extension
    )
    suffixed = tuple(command + extension for extension in extensions)
    # CPython shutil.which's X_OK policy admits the direct spelling only
    # when it already has a PATHEXT suffix. A sibling POSIX launcher must not
    # shadow the executable Windows command (for example npm beside npm.cmd).
    if any(
        command.casefold().endswith(extension.casefold()) for extension in extensions
    ):
        return (command, *suffixed)
    return suffixed


def executable_search_directories(
    *, environment: Mapping[str, str], cwd: Path, windows: bool | None = None
) -> tuple[Path, ...]:
    """Project PATH and Windows implicit-cwd policy without consulting ambient env.

    An absent or empty captured PATH disables PATH lookup. Explicit executable
    paths remain available. Empty components in a nonempty PATH name cwd.
    """
    if windows is None:
        windows = os.name == "nt"
    environment = ProcessEnvironment(environment)
    path = executable_environment_value(environment, "PATH")
    if not path:
        return ()
    roots: list[Path] = []
    if windows and "NoDefaultCurrentDirectoryInExePath" not in environment:
        roots.append(cwd)
    for raw in path.split(";" if windows else ":"):
        root = Path(raw.strip('"') if windows else raw) if raw else cwd
        roots.append(root if root.is_absolute() else cwd / root)
    return tuple(dict.fromkeys(roots))


# Windows error for a path through a junction or symlink the process may not
# follow (redirection trust; for example any user-created junction under SSH).
_ERROR_UNTRUSTED_MOUNT_POINT = 448


def is_searchable_executable(candidate: Path) -> bool:
    """Whether the OS executable search could select ``candidate``.

    The OS search skips a directory it cannot traverse and goes on: execvp
    skips EACCES, and Windows SearchPathW skips an untrusted mount point. So
    does this search; any other error still raises.
    """
    try:
        return candidate.is_file() and os.access(candidate, os.F_OK | os.X_OK)
    except PermissionError:
        return False
    except OSError as error:
        if getattr(error, "winerror", None) == _ERROR_UNTRUSTED_MOUNT_POINT:
            return False
        raise


def executable_candidates(
    command: str, *, environment: Mapping[str, str], cwd: Path | None = None
) -> Iterator[Path]:
    """Yield search-ordered executables without running or modifying any of them.

    While a build records its inputs, each searched directory and each yielded
    executable is reported to the build record: a program added to an earlier
    directory, or a retargeted link, selects another executable.
    """
    if not command or "\x00" in command:
        return
    from molt import input_report

    recording = input_report.active()
    cwd = Path.cwd() if cwd is None else cwd
    candidate = expand_user_path(command, environment=environment)
    if candidate.is_absolute() or any(separator in command for separator in "/\\"):
        candidate = candidate if candidate.is_absolute() else cwd / candidate
        directories = (candidate.parent,)
        command = candidate.name
    else:
        directories = executable_search_directories(environment=environment, cwd=cwd)
    names = executable_name_candidates(command, environment=environment)
    for directory in directories:
        if recording:
            input_report.report_directory(directory)
        for name in names:
            candidate = directory / name
            if is_searchable_executable(candidate):
                selected = lexical_executable_path(candidate)
                if recording:
                    input_report.report_data_file(selected)
                yield selected


def find_executable(
    command: str, *, environment: Mapping[str, str], cwd: Path | None = None
) -> Path | None:
    """Select one executable using only explicit paths or captured search inputs."""
    return next(executable_candidates(command, environment=environment, cwd=cwd), None)


def resolve_executable(
    command: str,
    *,
    environment: Mapping[str, str],
    label: str,
    cwd: Path | None = None,
) -> Path:
    """Resolve an explicit path or one PATH-selected executable exactly once."""

    if not command or "\x00" in command:
        raise ValueError(f"{label} command is invalid")
    candidate = expand_user_path(command, environment=environment)
    if not candidate.is_absolute() and not any(
        separator in command for separator in "/\\"
    ):
        selected = find_executable(command, environment=environment, cwd=cwd)
        if selected is None:
            raise ValueError(f"{label} is unavailable: {command}")
        candidate = Path(selected)
    elif not candidate.is_absolute() and cwd is not None:
        candidate = cwd / candidate
    return _executable_paths(candidate, label=label)[0]


def split_native_command(value: str) -> tuple[str, ...]:
    """Parse a host command using its actual native quoting authority."""
    if os.name != "nt":
        return tuple(shlex.split(value))

    # CommandLineToArgvW is the Windows command-line grammar used by native
    # launchers. shlex's POSIX and non-POSIX modes both mis-handle valid quoted
    # Windows paths in edge cases, so use the platform authority directly.
    import ctypes

    argc = ctypes.c_int()
    shell32 = ctypes.WinDLL("shell32", use_last_error=True)
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    command_line_to_argv = shell32.CommandLineToArgvW
    command_line_to_argv.argtypes = [ctypes.c_wchar_p, ctypes.POINTER(ctypes.c_int)]
    command_line_to_argv.restype = ctypes.POINTER(ctypes.c_wchar_p)
    argv = command_line_to_argv(value, ctypes.byref(argc))
    if not argv:
        raise ValueError(
            f"invalid Windows command line (error {ctypes.get_last_error()})"
        )
    kernel32.LocalFree.argtypes = [ctypes.c_void_p]
    kernel32.LocalFree.restype = ctypes.c_void_p
    try:
        return tuple(argv[index] for index in range(argc.value))
    finally:
        kernel32.LocalFree(argv)


def resolve_explicit_tool_command(
    raw_command: str,
    *,
    label: str,
    environment: Mapping[str, str] | None = None,
    cwd: Path | None = None,
) -> tuple[str, ...]:
    """Resolve an exact tool path or an explicitly configured command.

    Existing pathnames take precedence over command-word parsing, including
    unquoted paths containing spaces. Preserve the lexical driver entrypoint;
    content identity is captured separately and must not rewrite argv[0].
    """
    environment = os.environ if environment is None else environment
    cwd = Path.cwd() if cwd is None else cwd
    if not raw_command or "\x00" in raw_command:
        raise ValueError(f"{label} is empty or contains a NUL byte")

    def path_like(value: str) -> bool:
        return Path(value).is_absolute() or "/" in value or "\\" in value

    def absolute(value: str) -> Path:
        path = expand_user_path(value, environment=environment)
        return lexical_executable_path(path if path.is_absolute() else cwd / path)

    if path_like(raw_command):
        direct_path = absolute(raw_command)
        if direct_path.is_file():
            return (str(direct_path),)
    # Preserve literal filesystem spelling above. Command delimiters are not
    # argv[0]; Windows otherwise emits an empty first argument for leading
    # whitespace and selects the current executable for an empty command.
    command_text = raw_command.lstrip(" \t\r\n")
    if not command_text:
        raise ValueError(f"{label} is empty")
    try:
        argv = split_native_command(command_text)
    except ValueError as exc:
        raise ValueError(f"{label} is not a valid shell command: {exc}") from exc
    if not argv or not argv[0]:
        raise ValueError(f"{label} is empty")
    executable = argv[0]
    if path_like(executable):
        path = absolute(executable)
        if not path.is_file():
            raise ValueError(f"{label} executable not found: {executable}")
    else:
        selected = find_executable(executable, environment=environment, cwd=cwd)
        if selected is None:
            raise ValueError(f"{label} executable not found on PATH: {executable}")
        path = absolute(str(selected))
    return (str(path), *argv[1:])


def _capture_executable_content(
    path: Path,
    *,
    label: str,
) -> tuple[Path, StableRegularFileIdentity, bytes]:
    """Capture executable digest and header together, including entrypoint fences."""
    entrypoint, resolved = _executable_paths(path, label=label)
    before_entry = _stat_identity(entrypoint.lstat())
    with open_stable_regular_file(resolved, label=label) as opened:
        identity = stable_regular_file_handle_identity(opened, label=label)
        header = opened.stream.read(4)
        final_entrypoint, final_resolved = _executable_paths(entrypoint, label=label)
        if (
            final_entrypoint != entrypoint
            or final_resolved != resolved
            or _stat_identity(entrypoint.lstat()) != before_entry
        ):
            raise StableRegularFileChangedError(
                f"{label} entrypoint changed during capture: {entrypoint}"
            )
    return entrypoint, identity, header


def _stable_file_content(
    path: Path, *, label: str
) -> tuple[Path, Path, int, str, bytes]:
    """Snapshot executable bytes and header through their shared owned capture."""
    lexical, identity, header = _capture_executable_content(path, label=label)
    return lexical, identity.path, identity.size, identity.sha256, header


_DIGEST_CHUNK_BYTES = 256 * 1024


def _captured_file_chunks(
    stream: BinaryIO, *, size: int, chunk_bytes: int
) -> Iterator[bytes]:
    """Consume exactly one admitted extent, probing one byte for growth."""
    consumed = 0
    while True:
        chunk = stream.read(min(chunk_bytes, size - consumed + 1))
        if not chunk:
            if consumed != size:
                raise StableRegularFileChangedError(
                    "file size changed below its captured extent while reading"
                )
            return
        consumed += len(chunk)
        if consumed > size:
            raise StableRegularFileChangedError(
                "file size changed beyond its captured extent while reading"
            )
        yield chunk
        del chunk


def iter_stable_regular_file_chunks(
    opened: StableRegularFileHandle, *, chunk_bytes: int = _DIGEST_CHUNK_BYTES
) -> Iterator[bytes]:
    """Read a newly opened or rewound file without following concurrent growth.

    Exhaust this iterator inside the existing open_stable_regular_file context:
    its owner still checks pathname, handle and change-time identity on exit.
    No over-extent chunk reaches the consumer, including a staging destination.
    """
    if type(chunk_bytes) is not int or chunk_bytes <= 0:
        raise ValueError("stable file chunk size must be a positive integer")
    yield from _captured_file_chunks(
        opened.stream, size=opened.stat.st_size, chunk_bytes=chunk_bytes
    )


def _sha256_stream(stream: BinaryIO, *, max_bytes: int) -> str:
    """Hash exactly the captured file size using the shared finite traversal."""
    digest = hashlib.sha256()
    for chunk in _captured_file_chunks(
        stream, size=max_bytes, chunk_bytes=_DIGEST_CHUNK_BYTES
    ):
        digest.update(chunk)
        del chunk
    return digest.hexdigest()


def stable_file_sha256(path: Path, *, label: str) -> str:
    """Hash a regular file while proving its pathname and open handle stayed stable."""

    return _stable_file_content(path, label=label)[3]


def executable_content_identity(path: Path, *, label: str) -> dict[str, str | int]:
    """Return the canonical, path-independent identity of executable file bytes."""

    lexical, resolved, size, digest, _header = _stable_file_content(path, label=label)
    return {
        "entrypoint": _portable_filename(lexical.name),
        "content_filename": _portable_filename(resolved.name),
        "size": size,
        "sha256": digest,
    }


def stable_file_content_identity(path: Path, *, label: str) -> dict[str, str | int]:
    """Return one path-independent identity for a stable, non-indirect file."""

    lexical, resolved, size, digest, _header = _stable_file_content(path, label=label)
    if lexical != resolved:
        raise ValueError(
            f"{label} must not be a symlink or path indirection: {lexical}"
        )
    return {
        "filename": _portable_filename(lexical.name),
        "size": size,
        "sha256": digest,
    }


@dataclass(frozen=True, slots=True)
class StableRegularFileHandle:
    path: Path
    stream: BufferedReader
    stat: os.stat_result
    content_change_time_ns: int


@contextmanager
def open_stable_regular_file(
    path: Path,
    *,
    label: str,
    expected_path_stat: os.stat_result | None = None,
    observed: StableRegularFileVersion | None = None,
) -> Generator[StableRegularFileHandle]:
    """Open one direct regular file, optionally bound to a no-follow snapshot row.

    Supplied metadata replaces only the opening path lookup. The freshly opened
    handle and path must agree with it before bytes are read; all closing path,
    handle and content-change-time fences remain mandatory. On Windows the
    descriptor excludes writes and deletion until this context closes; native
    ChangeTime alone cannot distinguish writes within one filesystem clock tick.
    """

    lexical = path.expanduser().absolute()
    try:
        before_path = (
            lexical.lstat() if expected_path_stat is None else expected_path_stat
        )
    except OSError as exc:
        raise StableRegularFileError(f"{label} is unavailable: {lexical}") from exc
    if not stat.S_ISREG(before_path.st_mode) or metadata_is_link_like(before_path):
        raise StableRegularFileError(
            f"{label} is not one stable regular file: {lexical}"
        )
    try:
        descriptor = open_stable_read_descriptor(lexical)
    except OSError as exc:
        if getattr(exc, "winerror", None) in (32, 33):
            raise StableRegularFileChangedError(
                f"{label} changed or has a conflicting writer before identity read: "
                f"{lexical}"
            ) from exc
        try:
            current_path = lexical.lstat()
        except OSError:
            current_path = None
        if current_path is None or _stat_identity(current_path) != _stat_identity(
            before_path
        ):
            raise StableRegularFileChangedError(
                f"{label} changed before identity read: {lexical}"
            ) from exc
        raise StableRegularFileError(
            f"{label} could not be opened as one direct regular file: {lexical}"
        ) from exc
    try:
        stream = os.fdopen(descriptor, "rb")
    except BaseException:
        os.close(descriptor)
        raise
    with stream:
        try:
            before_handle = os.fstat(stream.fileno())
            before_change = content_change_time_ns_from_fd(
                stream.fileno(),
                before_handle,
            )
            opened_path = lexical.lstat()
        except OSError as exc:
            raise StableRegularFileChangedError(
                f"{label} changed before identity read: {lexical}"
            ) from exc
        if before_change is None:
            raise StableRegularFileError(
                f"{label} cannot establish direct-file change-time identity: {lexical}"
            )
        if (
            not stat.S_ISREG(before_handle.st_mode)
            or metadata_is_link_like(before_handle)
            or metadata_is_link_like(opened_path)
            or _stat_identity(opened_path) != _stat_identity(before_path)
            or _path_handle_identity(before_handle)
            != _path_handle_identity(opened_path)
        ):
            raise StableRegularFileChangedError(
                f"{label} changed before identity read: {lexical}"
            )
        opened = StableRegularFileHandle(
            path=lexical,
            stream=stream,
            stat=before_handle,
            content_change_time_ns=before_change,
        )
        if observed is not None and (
            opened.path != observed.path
            or _stat_identity(before_handle) != observed._stat_identity
            or before_change != observed._content_change_time_ns
        ):
            raise StableRegularFileChangedError(
                f"{label} changed since identity capture: {lexical}"
            )
        try:
            yield opened
        except OSError as exc:
            # Win32 reports sharing violations directly; CPython's CRT-backed
            # open() can report only EACCES. Bind that case to this pathname so
            # an unrelated permission failure keeps its original diagnostic.
            same_path_denied = (
                os.name == "nt"
                and exc.filename is not None
                and os.path.normcase(os.path.abspath(exc.filename))
                == os.path.normcase(os.path.abspath(lexical))
                and (
                    getattr(exc, "winerror", None) in (32, 33)
                    or isinstance(exc, PermissionError)
                )
            )
            if same_path_denied:
                raise StableRegularFileChangedError(
                    f"{label} changed or attempted a conflicting write/delete "
                    f"during identity read: {lexical}"
                ) from exc
            raise
        try:
            after_handle = os.fstat(stream.fileno())
            after_change = content_change_time_ns_from_fd(
                stream.fileno(),
                after_handle,
            )
            after_path = lexical.lstat()
        except OSError as exc:
            raise StableRegularFileChangedError(
                f"{label} changed during identity read: {lexical}"
            ) from exc
        if after_change is None:
            raise StableRegularFileError(
                f"{label} cannot establish direct-file change-time identity: {lexical}"
            )
        if (
            metadata_is_link_like(after_handle)
            or metadata_is_link_like(after_path)
            or _stat_identity(before_path) != _stat_identity(after_path)
            or _stat_identity(before_handle) != _stat_identity(after_handle)
            or _path_handle_identity(after_path) != _path_handle_identity(after_handle)
            or before_change != after_change
        ):
            raise StableRegularFileChangedError(
                f"{label} changed during identity read: {lexical}"
            )


def _stable_regular_file_snapshot(
    path: Path,
    *,
    label: str,
    hash_content: bool,
    expected_path_stat: os.stat_result | None = None,
) -> tuple[Path, os.stat_result, int, str | None]:
    """Read one direct regular file's stable open-handle identity."""

    try:
        with open_stable_regular_file(
            path, label=label, expected_path_stat=expected_path_stat
        ) as opened:
            digest = (
                stable_regular_file_handle_identity(opened, label=label).sha256
                if hash_content
                else None
            )
    except OSError as exc:
        operation = "hashed" if hash_content else "verified"
        lexical = path.expanduser().absolute()
        raise ValueError(f"{label} could not be {operation}: {lexical}") from exc
    return opened.path, opened.stat, opened.content_change_time_ns, digest


def _regular_file_identity(
    path: Path, metadata: os.stat_result, change_time_ns: int, digest: str
) -> StableRegularFileIdentity:
    return StableRegularFileIdentity(
        path=path,
        size=metadata.st_size,
        sha256=digest,
        _stat_identity=_stat_identity(metadata),
        _content_change_time_ns=change_time_ns,
    )


def stable_regular_file_handle_identity(
    opened: StableRegularFileHandle, *, label: str, max_bytes: int | None = None
) -> StableRegularFileIdentity:
    """Hash no more than the captured size, then rewind it for its consumer.

    The caller retains the enclosing open_stable_regular_file context through
    consumption. This is the same content authority as a pathname capture;
    callers that already own a descriptor must not detach and reopen the path.
    """
    if max_bytes is not None and (
        type(max_bytes) is not int or max_bytes <= 0 or opened.stat.st_size > max_bytes
    ):
        raise ValueError(f"{label} exceeds size limit: {opened.path}")
    opened.stream.seek(0)
    digest = _sha256_stream(opened.stream, max_bytes=opened.stat.st_size)
    if opened.stream.tell() != opened.stat.st_size:
        raise StableRegularFileChangedError(
            f"{label} size changed during capture: {opened.path}"
        )
    opened.stream.seek(0)
    return _regular_file_identity(
        opened.path, opened.stat, opened.content_change_time_ns, digest
    )


def verify_stable_regular_file_content(
    identity: StableRegularFileIdentity, *, sha256: str, size: int, label: str
) -> None:
    """Bind a digest of bytes consumed under an owned read to its old identity.

    This comparison performs no I/O. The caller supplies the digest and length
    of the actual read buffer, mapped view or copied stream, before closing its
    open_stable_regular_file context. Metadata-only polling remains separate.
    """
    if size != identity.size or sha256 != identity.sha256:
        raise StableRegularFileChangedError(
            f"{label} content changed since identity capture: {identity.path}"
        )


def capture_stable_regular_file(
    path: Path, *, label: str, max_bytes: int | None = None
) -> tuple[StableRegularFileIdentity, bytes]:
    """Capture bytes and their mutation identity from one stable no-follow read."""
    with open_stable_regular_file(path, label=label) as opened:
        if max_bytes is not None and (
            type(max_bytes) is not int
            or max_bytes <= 0
            or opened.stat.st_size > max_bytes
        ):
            raise ValueError(f"{label} exceeds size limit: {path}")
        # Read one byte beyond the admitted length to detect growth without an
        # unbounded allocation if a POSIX writer extends the file concurrently.
        data = opened.stream.read(opened.stat.st_size + 1)
        if max_bytes is not None and len(data) > max_bytes:
            raise ValueError(f"{label} exceeds size limit: {path}")
        if len(data) != opened.stat.st_size:
            raise StableRegularFileChangedError(
                f"{label} size changed during capture: {opened.path}"
            )
        identity = _regular_file_identity(
            opened.path,
            opened.stat,
            opened.content_change_time_ns,
            hashlib.sha256(data).hexdigest(),
        )
    return identity, data


def stable_regular_file_identity(
    path: Path,
    *,
    label: str,
    expected_path_stat: os.stat_result | None = None,
) -> StableRegularFileIdentity:
    """Hash one direct regular file and retain its cheap mutation identity."""

    lexical, file_stat, change_time_ns, digest = _stable_regular_file_snapshot(
        path,
        label=label,
        hash_content=True,
        expected_path_stat=expected_path_stat,
    )
    if digest is None:
        raise RuntimeError("stable regular-file identity omitted its content digest")
    return _regular_file_identity(lexical, file_stat, change_time_ns, digest)


class FileDigestCache:
    """Stat-keyed SHA-256 digests that processes on one host reuse.

    A recorded digest is reused only for a file whose (device, inode, mode,
    size, mtime_ns, ctime_ns) equals the observation it was recorded with, and
    only digests of files whose timestamps were already older than the racy
    window are recorded. POSIX kernels update ctime on every write and callers
    cannot set it, so an equal token cannot hide a write. Windows ChangeTime
    can repeat within one clock tick, so Windows never consults the cache.
    Rows unused for the cache's age budget are deleted.
    """

    def __init__(self, store: Path, *, max_age_ns: int) -> None:
        self.store = store
        self._max_age_ns = max_age_ns
        self._started_ns = time.time_ns()
        self._recorded: dict[str, tuple[_StatToken, str, int]] | None = None
        self._pending: dict[str, tuple[_StatToken, str]] = {}

    def _connect(self) -> sqlite3.Connection:
        self.store.parent.mkdir(parents=True, exist_ok=True)
        connection = sqlite3.connect(self.store, timeout=60)
        connection.execute("PRAGMA journal_mode=WAL")
        connection.execute(
            "CREATE TABLE IF NOT EXISTS digests (path TEXT PRIMARY KEY,"
            " dev INTEGER NOT NULL, ino INTEGER NOT NULL, mode INTEGER NOT NULL,"
            " size INTEGER NOT NULL, mtime_ns INTEGER NOT NULL,"
            " ctime_ns INTEGER NOT NULL, sha256 TEXT NOT NULL,"
            " recorded_ns INTEGER NOT NULL)"
        )
        connection.execute(
            "CREATE INDEX IF NOT EXISTS digests_recorded ON digests (recorded_ns)"
        )
        return connection

    def _rows(self) -> dict[str, tuple[_StatToken, str, int]]:
        if self._recorded is None:
            with closing(self._connect()) as connection:
                self._recorded = {
                    row[0]: (
                        (row[1], row[2], row[3], row[4], row[5], row[6]),
                        row[7],
                        row[8],
                    )
                    for row in connection.execute(
                        "SELECT path, dev, ino, mode, size, mtime_ns, ctime_ns,"
                        " sha256, recorded_ns FROM digests"
                    )
                }
        return self._recorded

    def identity(self, path: Path, *, label: str) -> StableRegularFileIdentity:
        """This file's stable identity; hash only without a current digest."""
        if os.name == "nt":
            return stable_regular_file_identity(path, label=label)
        try:
            metadata = path.lstat()
        except OSError as exc:
            raise StableRegularFileError(f"{label} is unavailable: {path}") from exc
        if not stat.S_ISREG(metadata.st_mode):
            raise StableRegularFileError(
                f"{label} is not one stable regular file: {path}"
            )
        recorded = self.lookup(path, expected_path_stat=metadata, label=label)
        if recorded is not None:
            return recorded
        identity = stable_regular_file_identity(
            path, label=label, expected_path_stat=metadata
        )
        self.remember(identity)
        return identity

    def lookup(
        self, path: Path, *, expected_path_stat: os.stat_result, label: str
    ) -> StableRegularFileIdentity | None:
        """Look up a captured snapshot; its owner retains the closing fence.

        Never select newer path metadata here. A miss must be hashed against
        the same supplied observation, and a hit must still pass its owner's
        current-path and publication fences.
        """
        if os.name == "nt":
            return None
        if not stat.S_ISREG(expected_path_stat.st_mode):
            raise StableRegularFileError(
                f"{label} is not one stable regular file: {path}"
            )
        key = os.fspath(path)
        token = _stat_identity(expected_path_stat)
        recorded = self._rows().get(key)
        if recorded is not None and recorded[0] == token:
            if recorded[2] < self._started_ns - self._max_age_ns // 2:
                self._pending[key] = (token, recorded[1])
            return _regular_file_identity(
                path, expected_path_stat, expected_path_stat.st_ctime_ns, recorded[1]
            )
        return None

    def remember(self, identity: StableRegularFileIdentity) -> None:
        """Stage one completed capture under the existing racy-window policy."""
        if os.name != "nt" and (
            max(identity._stat_identity[4], identity._stat_identity[5])
            < self._started_ns - DIGEST_RACY_WINDOW_NS
        ):
            self._pending[os.fspath(identity.path)] = (
                identity._stat_identity,
                identity.sha256,
            )

    def flush(self) -> None:
        """Record this process's new digests and drop rows past the age budget."""
        if not self._pending:
            return
        rows = [
            (path, *token, sha256, self._started_ns)
            for path, (token, sha256) in self._pending.items()
        ]
        with closing(self._connect()) as connection, connection:
            connection.executemany(
                "INSERT OR REPLACE INTO digests VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                rows,
            )
            connection.execute(
                "DELETE FROM digests WHERE recorded_ns < ?",
                (self._started_ns - self._max_age_ns,),
            )
        recorded = self._rows()
        for path, (token, sha256) in self._pending.items():
            recorded[path] = (token, sha256, self._started_ns)
        self._pending.clear()


_FILE_DIGEST_CACHES: dict[Path, FileDigestCache] = {}


def file_digest_cache(store: Path, *, max_age_ns: int) -> FileDigestCache:
    """This process's digest cache over one store."""
    cache = _FILE_DIGEST_CACHES.get(store)
    if cache is None:
        cache = _FILE_DIGEST_CACHES[store] = FileDigestCache(
            store, max_age_ns=max_age_ns
        )
    return cache


def stable_regular_file_version(path: Path, *, label: str) -> StableRegularFileVersion:
    """Capture the shared no-follow handle/mutation token without reading bytes."""
    lexical, metadata, change_time_ns, _ = _stable_regular_file_snapshot(
        path, label=label, hash_content=False
    )
    return StableRegularFileVersion(
        lexical, metadata.st_size, _stat_identity(metadata), change_time_ns
    )


_STABLE_SNAPSHOT_CHUNK_BYTES = 1024 * 1024
_STABLE_SNAPSHOT_MAX_PREFIX_BYTES = 64 * 1024


def snapshot_stable_regular_file(
    source: Path,
    snapshot: Path,
    *,
    label: str,
    capture_prefix_bytes: int = 0,
    max_bytes: int | None = None,
) -> StableRegularFileSnapshot:
    """Stream one direct stable file into one exclusive attested snapshot.

    A caller's optional byte allowance is checked before output creation. Every
    copy is bounded by the captured source size, including growth after opening.
    """
    if max_bytes is not None and (type(max_bytes) is not int or max_bytes < 0):
        raise ValueError("stable snapshot byte limit must be a nonnegative integer")

    if not 0 <= capture_prefix_bytes <= _STABLE_SNAPSHOT_MAX_PREFIX_BYTES:
        raise ValueError(
            "stable snapshot prefix length must be between zero and "
            f"{_STABLE_SNAPSHOT_MAX_PREFIX_BYTES} bytes"
        )
    snapshot = snapshot.expanduser().absolute()
    snapshot.parent.mkdir(parents=True, exist_ok=True)
    hasher = hashlib.sha256()
    prefix = bytearray()
    owned_snapshot_identity: os.stat_result | None = None
    try:
        with open_stable_regular_file(source, label=label) as opened:
            if max_bytes is not None and opened.stat.st_size > max_bytes:
                raise StableRegularFileSnapshotError(
                    f"{label} exceeds snapshot byte limit: {opened.stat.st_size} > {max_bytes}"
                )
            if snapshot == opened.path:
                raise StableRegularFileError(
                    f"{label} snapshot must differ from its source: {snapshot}"
                )
            flags = (
                os.O_WRONLY
                | os.O_CREAT
                | os.O_EXCL
                | getattr(os, "O_BINARY", 0)
                | getattr(os, "O_NOINHERIT", 0)
            )
            descriptor = os.open(
                snapshot,
                flags,
                stat.S_IMODE(opened.stat.st_mode),
            )
            try:
                owned_snapshot_identity = os.fstat(descriptor)
                destination = os.fdopen(descriptor, "wb")
            except BaseException:
                os.close(descriptor)
                raise
            with destination:
                copied = 0
                for chunk in iter_stable_regular_file_chunks(
                    opened, chunk_bytes=_STABLE_SNAPSHOT_CHUNK_BYTES
                ):
                    copied += len(chunk)
                    hasher.update(chunk)
                    written = destination.write(chunk)
                    if written != len(chunk):
                        raise OSError(
                            f"short write while snapshotting {label}: "
                            f"{written}/{len(chunk)} bytes"
                        )
                    remaining_prefix = capture_prefix_bytes - len(prefix)
                    if remaining_prefix > 0:
                        prefix.extend(chunk[:remaining_prefix])
        snapshot.chmod(stat.S_IMODE(opened.stat.st_mode))
        try:
            snapshot_identity = stable_regular_file_identity(
                snapshot,
                label=f"snapshotted {label}",
            )
        except StableRegularFileChangedError as exc:
            raise StableRegularFileSnapshotError(
                f"{label} snapshot changed during independent attestation: {snapshot}"
            ) from exc
    except BaseException:
        unlink_owned_file(snapshot, owned_snapshot_identity)
        raise
    digest = hasher.hexdigest()
    if (
        opened.stat.st_size != snapshot_identity.size
        or digest != snapshot_identity.sha256
    ):
        unlink_owned_file(snapshot, owned_snapshot_identity)
        raise StableRegularFileSnapshotError(
            f"{label} snapshot content changed after its source stream: {snapshot}"
        )
    source_identity = _regular_file_identity(
        opened.path,
        opened.stat,
        opened.content_change_time_ns,
        digest,
    )
    if owned_snapshot_identity is None:
        raise RuntimeError("stable snapshot lost its destination ownership identity")
    return StableRegularFileSnapshot(
        source=source_identity,
        snapshot=snapshot_identity,
        prefix=bytes(prefix),
    )


def verify_stable_regular_file_identity(
    identity: StableRegularFileVersion,
    *,
    label: str,
    expected_path_stat: os.stat_result | None = None,
    hash_content: bool = False,
) -> None:
    """Compare current handle metadata with a previously observed generation.

    Detached observations do not retain read custody. Metadata polling does not
    revalidate bytes; content-cache admission must explicitly request hashing.
    """
    if hash_content and not isinstance(identity, StableRegularFileIdentity):
        raise ValueError(f"{label} lacks a captured content digest: {identity.path}")
    _path, file_stat, change_time_ns, digest = _stable_regular_file_snapshot(
        identity.path,
        label=label,
        hash_content=hash_content,
        expected_path_stat=expected_path_stat,
    )
    if (
        _stat_identity(file_stat) != identity._stat_identity
        or change_time_ns != identity._content_change_time_ns
    ):
        raise ValueError(f"{label} changed since identity capture: {identity.path}")
    if hash_content:
        assert isinstance(identity, StableRegularFileIdentity) and digest is not None
        verify_stable_regular_file_content(
            identity, sha256=digest, size=file_stat.st_size, label=label
        )


def verify_stable_regular_file_metadata(
    identity: StableRegularFileVersion,
    *,
    label: str,
    expected_path_stat: os.stat_result | None = None,
) -> None:
    """Compare one POSIX path stat with a captured generation.

    On POSIX the path's stat carries the same (device, inode, mode, size,
    mtime, ctime) a fresh handle would report, so no open is needed. Windows
    needs the handle's ChangeTime: use verify_stable_regular_file_identity.
    """
    if os.name == "nt":
        raise ValueError("Windows file metadata needs a handle ChangeTime query")
    if expected_path_stat is not None and (
        _stat_identity(expected_path_stat) != identity._stat_identity
        or expected_path_stat.st_ctime_ns != identity._content_change_time_ns
    ):
        raise ValueError(f"{label} changed since identity capture: {identity.path}")
    try:
        metadata = identity.path.lstat()
    except OSError as exc:
        raise ValueError(f"{label} is unavailable: {identity.path}") from exc
    if (
        _stat_identity(metadata) != identity._stat_identity
        or metadata.st_ctime_ns != identity._content_change_time_ns
    ):
        raise ValueError(f"{label} changed since identity capture: {identity.path}")


def read_stable_regular_file(
    identity: StableRegularFileIdentity, *, label: str, max_bytes: int | None = None
) -> bytes:
    """Read once and validate the returned bytes against their captured digest.

    Metadata observations fence the open, but cannot attest content across a
    closed-handle interval. Hash the already-read buffer under this owned read;
    no second read, descriptor or retained content cache is needed.
    """

    if max_bytes is not None and (
        type(max_bytes) is not int or max_bytes <= 0 or identity.size > max_bytes
    ):
        raise ValueError(f"{label} exceeds size limit: {identity.path}")
    with open_stable_regular_file(
        identity.path, label=label, observed=identity
    ) as opened:
        data = opened.stream.read(identity.size + 1)
        verify_stable_regular_file_content(
            identity,
            sha256=hashlib.sha256(data).hexdigest(),
            size=len(data),
            label=label,
        )
    return data


@contextmanager
def stable_executable_probe(
    path: Path,
    *,
    label: str,
    identity: StableRegularFileIdentity | None = None,
) -> Generator[tuple[Path, StableRegularFileIdentity]]:
    """Bind a probe to a cold-captured or already-attested executable generation."""
    entrypoint, resolved = _executable_paths(path, label=label)
    before_entry = _stat_identity(entrypoint.lstat())
    if identity is None:
        captured_entrypoint, identity, _header = _capture_executable_content(
            path, label=label
        )
        if captured_entrypoint != entrypoint or identity.path != resolved:
            raise StableRegularFileChangedError(
                f"{label} entrypoint changed before probe: {entrypoint}"
            )
    else:
        if resolved != identity.path:
            raise StableRegularFileChangedError(
                f"{label} entrypoint resolves to a different generation: {entrypoint}"
            )
        verify_stable_regular_file_identity(identity, label=label)
    try:
        yield entrypoint, identity
    finally:
        final_entrypoint, final_resolved = _executable_paths(entrypoint, label=label)
        if (
            final_entrypoint != entrypoint
            or final_resolved != resolved
            or _stat_identity(entrypoint.lstat()) != before_entry
        ):
            raise StableRegularFileChangedError(
                f"{label} entrypoint changed during probe: {entrypoint}"
            )
        verify_stable_regular_file_identity(identity, label=label)


def stable_regular_file_content_identity(
    path: Path,
    *,
    label: str,
) -> dict[str, str | int]:
    """Return path-independent content identity for one stable direct file."""

    identity = stable_regular_file_identity(path, label=label)
    return {
        "filename": _portable_filename(identity.path.name),
        "size": identity.size,
        "sha256": identity.sha256,
    }


def _native_executable_header(header: bytes) -> bool:
    if header.startswith(b"MZ") or header == b"\x7fELF":
        return True
    return header in {
        b"\xfe\xed\xfa\xce",
        b"\xfe\xed\xfa\xcf",
        b"\xce\xfa\xed\xfe",
        b"\xcf\xfa\xed\xfe",
        b"\xca\xfe\xba\xbe",
        b"\xbe\xba\xfe\xca",
        b"\xca\xfe\xba\xbf",
        b"\xbf\xba\xfe\xca",
    }


@contextmanager
def stable_native_executable_probe(
    path: Path, *, label: str
) -> Generator[tuple[Path, StableRegularFileIdentity]]:
    """Bind native executable admission and a probe to one captured generation."""
    entrypoint, identity, header = _capture_executable_content(path, label=label)
    if not _native_executable_header(header):
        raise ValueError(
            f"{label} must be a native executable, not a script or delegating wrapper: "
            f"{identity.path}"
        )
    with stable_executable_probe(entrypoint, label=label, identity=identity):
        yield entrypoint, identity


def native_executable_content_identity(
    path: Path,
    *,
    label: str,
) -> dict[str, str | int]:
    """Return content identity only when the selected file is a native executable."""

    with stable_native_executable_probe(path, label=label) as (entrypoint, identity):
        return {
            "entrypoint": _portable_filename(entrypoint.name),
            "content_filename": _portable_filename(identity.path.name),
            "size": identity.size,
            "sha256": identity.sha256,
        }


def probe_executable(
    path: Path,
    *,
    version_arguments: Sequence[Sequence[str]],
    environment: Mapping[str, str],
    label: str,
    accepted_returncodes: frozenset[int] = frozenset({0}),
    version_pattern: re.Pattern[str] | None = None,
    version_patterns: Sequence[re.Pattern[str]] | None = None,
    timeout: float = 30.0,
) -> ExecutableIdentity:
    """Bind resolved bytes and one anchored semantic version probe."""

    if version_pattern is not None and version_patterns is not None:
        raise ValueError(f"{label} version identity has conflicting patterns")
    with stable_native_executable_probe(path, label=label) as (entrypoint, identity):
        resolved = identity.path
        version = ""
        for raw_arguments in version_arguments:
            arguments = tuple(raw_arguments)
            try:
                completed = subprocess.run(
                    [str(entrypoint), *arguments],
                    env=ProcessEnvironment(environment).child_block(),
                    check=False,
                    capture_output=True,
                    text=True,
                    encoding="utf-8",
                    errors="replace",
                    timeout=timeout,
                )
            except (OSError, subprocess.SubprocessError) as exc:
                raise ValueError(f"{label} version probe failed: {resolved}") from exc
            if completed.returncode not in accepted_returncodes:
                continue
            observed = "\n".join(
                part.strip().replace("\r\n", "\n")
                for part in (completed.stdout, completed.stderr)
                if part.strip()
            )
            if version_patterns is not None:
                matched_lines: list[str] = []
                for pattern in version_patterns:
                    match = pattern.search(observed)
                    if match is None:
                        matched_lines.clear()
                        break
                    matched_lines.append(match.group(0))
                observed = "\n".join(matched_lines)
            elif version_pattern is not None:
                match = version_pattern.search(observed)
                observed = match.group(0) if match is not None else ""
            if observed:
                version = observed
                break
        if not version:
            raise ValueError(f"{label} has no accepted version identity: {resolved}")
        return ExecutableIdentity(
            path=entrypoint,
            content_path=resolved,
            size=identity.size,
            sha256=identity.sha256,
            version=version,
        )
