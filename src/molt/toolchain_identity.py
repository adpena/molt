"""Exact executable identity shared by build, release, and runtime toolchains."""

from __future__ import annotations

from collections.abc import Iterator, Mapping, Sequence
from contextlib import contextmanager
from dataclasses import dataclass
import hashlib
from io import BufferedReader
import ntpath
import os
from pathlib import Path
import re
import shlex
import stat
import subprocess
from typing import BinaryIO

from molt.file_hashing import content_change_time_ns, content_change_time_ns_from_fd


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
class StableRegularFileIdentity:
    """Immutable content and mutation identity for one direct regular file."""

    path: Path
    size: int
    sha256: str
    _stat_identity: tuple[int, int, int, int, int, int]
    _content_change_time_ns: int


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


def _stat_identity(value: os.stat_result) -> tuple[int, int, int, int, int, int]:
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


def _stat_is_reparse_point(value: os.stat_result) -> bool:
    """Return whether platform file metadata marks path indirection."""

    reparse_attribute = getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", None)
    file_attributes = getattr(value, "st_file_attributes", None)
    return (
        reparse_attribute is not None
        and file_attributes is not None
        and bool(file_attributes & reparse_attribute)
    )


def _unlink_owned_file(path: Path, identity: os.stat_result | None) -> None:
    """Unlink only the file object created by the current operation."""

    if identity is None:
        return
    try:
        current = path.lstat()
    except FileNotFoundError:
        return
    if os.path.samestat(identity, current):
        path.unlink()


def _unlink_stable_file_identity(identity: StableRegularFileIdentity) -> None:
    """Unlink only a path that still names the attested file generation."""

    try:
        current = identity.path.lstat()
    except FileNotFoundError:
        return
    if _path_handle_identity(current) != _stored_path_handle_identity(
        identity._stat_identity
    ):
        return
    current_change_time_ns = content_change_time_ns(identity.path, current)
    if current_change_time_ns != identity._content_change_time_ns:
        return
    identity.path.unlink()


def _executable_paths(path: Path, *, label: str) -> tuple[Path, Path]:
    """Return the lexical entrypoint and its resolved regular-file content."""

    lexical = path.expanduser().absolute()
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


def executable_environment_value(
    environment: Mapping[str, str],
    key: str,
    default: str = "",
    *,
    windows: bool | None = None,
) -> str:
    """Read captured executable-selection inputs with host key semantics."""
    if windows is None:
        windows = os.name == "nt"
    if not windows:
        return environment.get(key, default)
    values = {
        value
        for name, value in environment.items()
        if name.casefold() == key.casefold()
    }
    if len(values) > 1:
        raise ValueError(f"conflicting captured environment spellings for {key}")
    return next(iter(values), default)


def expand_user_path(
    path: str | Path, *, environment: Mapping[str, str] | None = None
) -> Path:
    """Expand a user selector using the selected environment, never ambient keys.

    With no explicit mapping, retain pathlib's host behavior. POSIX named users
    (and a missing HOME) use the system account database, as pathlib does;
    Windows uses USERPROFILE or HOMEDRIVE/HOMEPATH and captured USERNAME.
    """
    raw = os.fspath(path)
    if not raw.startswith("~"):
        return Path(raw)
    if environment is None:
        return Path(raw).expanduser()
    separators = "/\\" if os.name == "nt" else "/"
    end = next(
        (index for index, char in enumerate(raw) if char in separators), len(raw)
    )
    user, suffix = raw[1:end], raw[end:]
    if os.name == "nt":
        home = executable_environment_value(environment, "USERPROFILE")
        if not home:
            homepath = executable_environment_value(environment, "HOMEPATH")
            if not homepath:
                raise ValueError("selected environment has no user home for tilde path")
            home = ntpath.join(
                executable_environment_value(environment, "HOMEDRIVE"), homepath
            )
        if user:
            current_user = executable_environment_value(environment, "USERNAME")
            if user != current_user:
                if ntpath.basename(home.rstrip("\\/")) != current_user:
                    raise ValueError(
                        "selected environment cannot resolve named user home"
                    )
                home = ntpath.join(ntpath.dirname(home.rstrip("\\/")), user)
    else:
        home = environment.get("HOME") if not user else None
        if home is None:
            import pwd

            try:
                home = (
                    pwd.getpwnam(user) if user else pwd.getpwuid(os.getuid())
                ).pw_dir
            except KeyError as exc:
                raise ValueError(
                    "cannot resolve system account for tilde path"
                ) from exc
    if "\0" in home:
        raise ValueError("selected user home contains a NUL byte")
    expanded = home + suffix if os.name == "nt" else home.rstrip(separators) + suffix
    if expanded.startswith("~"):
        raise ValueError("selected user home did not resolve tilde path")
    return Path(expanded or os.sep)


def executable_name_candidates(
    command: str, *, environment: Mapping[str, str], windows: bool | None = None
) -> tuple[str, ...]:
    """Expand executable suffixes solely from the captured PATHEXT policy."""
    if windows is None:
        windows = os.name == "nt"
    if not windows:
        return (command,)
    extensions = tuple(
        extension
        for extension in executable_environment_value(
            environment, "PATHEXT", ".COM;.EXE;.BAT;.CMD", windows=True
        ).split(";")
        if extension
    )
    if any(
        command.casefold().endswith(extension.casefold()) for extension in extensions
    ):
        return (command,)
    return (command, *(command + extension for extension in extensions))


def executable_search_directories(
    *, environment: Mapping[str, str], cwd: Path, windows: bool | None = None
) -> tuple[Path, ...]:
    """Project PATH and Windows implicit-cwd policy without consulting ambient env.

    An absent or empty captured PATH disables PATH lookup. Explicit executable
    paths remain available. Empty components in a nonempty PATH name cwd.
    """
    if windows is None:
        windows = os.name == "nt"
    path = executable_environment_value(environment, "PATH", windows=windows)
    if not path:
        return ()
    roots: list[Path] = []
    if windows and not any(
        key.casefold() == "nodefaultcurrentdirectoryinexepath" for key in environment
    ):
        roots.append(cwd)
    for raw in path.split(";" if windows else ":"):
        root = Path(raw.strip('"') if windows else raw) if raw else cwd
        roots.append(root if root.is_absolute() else cwd / root)
    return tuple(dict.fromkeys(roots))


def find_executable(
    command: str, *, environment: Mapping[str, str], cwd: Path | None = None
) -> Path | None:
    """Select one executable using only explicit paths or captured search inputs."""
    if not command or "\x00" in command:
        return None
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
        for name in names:
            candidate = directory / name
            if candidate.is_file() and os.access(candidate, os.F_OK | os.X_OK):
                return candidate.absolute()
    return None


def resolve_executable(
    command: str,
    *,
    environment: Mapping[str, str],
    label: str,
) -> Path:
    """Resolve an explicit path or one PATH-selected executable exactly once."""

    if not command or "\x00" in command:
        raise ValueError(f"{label} command is invalid")
    candidate = expand_user_path(command, environment=environment)
    if not candidate.is_absolute() and not any(
        separator in command for separator in "/\\"
    ):
        selected = find_executable(command, environment=environment)
        if selected is None:
            raise ValueError(f"{label} is unavailable: {command}")
        candidate = Path(selected)
    return _executable_paths(candidate, label=label)[0]


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
        return Path(os.path.abspath(path if path.is_absolute() else cwd / path))

    if path_like(raw_command):
        direct_path = absolute(raw_command)
        if direct_path.is_file():
            return (str(direct_path),)
    try:
        argv = shlex.split(raw_command, posix=os.name != "nt")
    except ValueError as exc:
        raise ValueError(f"{label} is not a valid shell command: {exc}") from exc
    if os.name == "nt":
        argv = [
            argument[1:-1]
            if len(argument) >= 2 and argument[0] == argument[-1] == '"'
            else argument
            for argument in argv
        ]
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


def _stable_file_content(
    path: Path,
    *,
    label: str,
) -> tuple[Path, Path, int, str, bytes]:
    """Snapshot a regular file while proving pathname and handle stability."""

    with stable_executable_probe(path, label=label) as (lexical, identity):
        with open_stable_regular_file(identity.path, label=label) as opened:
            header = opened.stream.read(4)
        return lexical, identity.path, identity.size, identity.sha256, header


_DIGEST_CHUNK_BYTES = 256 * 1024


def _sha256_stream(stream: BinaryIO) -> str:
    """SHA-256 of an open binary handle, read in bounded chunks."""

    digest = hashlib.sha256()
    while True:
        chunk = stream.read(_DIGEST_CHUNK_BYTES)
        if not chunk:
            break
        digest.update(chunk)
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
) -> Iterator[StableRegularFileHandle]:
    """Open one direct regular file without following path indirection."""

    lexical = path.expanduser().absolute()
    try:
        before_path = lexical.lstat()
    except OSError as exc:
        raise StableRegularFileError(f"{label} is unavailable: {lexical}") from exc
    if not stat.S_ISREG(before_path.st_mode) or _stat_is_reparse_point(before_path):
        raise StableRegularFileError(
            f"{label} is not one stable regular file: {lexical}"
        )
    flags = (
        os.O_RDONLY
        | getattr(os, "O_BINARY", 0)
        | getattr(os, "O_NOINHERIT", 0)
        | getattr(os, "O_NOFOLLOW", 0)
    )
    try:
        descriptor = os.open(lexical, flags)
    except OSError as exc:
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
            or _stat_is_reparse_point(before_handle)
            or _stat_is_reparse_point(opened_path)
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
        yield opened
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
            _stat_is_reparse_point(after_handle)
            or _stat_is_reparse_point(after_path)
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
) -> tuple[Path, os.stat_result, int, str | None]:
    """Read one direct regular file's stable open-handle identity."""

    try:
        with open_stable_regular_file(path, label=label) as opened:
            digest = _sha256_stream(opened.stream) if hash_content else None
    except OSError as exc:
        operation = "hashed" if hash_content else "verified"
        lexical = path.expanduser().absolute()
        raise ValueError(f"{label} could not be {operation}: {lexical}") from exc
    return opened.path, opened.stat, opened.content_change_time_ns, digest


def stable_regular_file_identity(
    path: Path,
    *,
    label: str,
) -> StableRegularFileIdentity:
    """Hash one direct regular file and retain its cheap mutation identity."""

    lexical, file_stat, change_time_ns, digest = _stable_regular_file_snapshot(
        path,
        label=label,
        hash_content=True,
    )
    if digest is None:
        raise RuntimeError("stable regular-file identity omitted its content digest")
    return StableRegularFileIdentity(
        path=lexical,
        size=file_stat.st_size,
        sha256=digest,
        _stat_identity=_stat_identity(file_stat),
        _content_change_time_ns=change_time_ns,
    )


_STABLE_SNAPSHOT_CHUNK_BYTES = 1024 * 1024
_STABLE_SNAPSHOT_MAX_PREFIX_BYTES = 64 * 1024


def snapshot_stable_regular_file(
    source: Path,
    snapshot: Path,
    *,
    label: str,
    capture_prefix_bytes: int = 0,
) -> StableRegularFileSnapshot:
    """Stream one direct stable file into one exclusive attested snapshot."""

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
                while chunk := opened.stream.read(_STABLE_SNAPSHOT_CHUNK_BYTES):
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
        _unlink_owned_file(snapshot, owned_snapshot_identity)
        raise
    digest = hasher.hexdigest()
    if (
        opened.stat.st_size != snapshot_identity.size
        or digest != snapshot_identity.sha256
    ):
        _unlink_owned_file(snapshot, owned_snapshot_identity)
        raise StableRegularFileSnapshotError(
            f"{label} snapshot content changed after its source stream: {snapshot}"
        )
    source_identity = StableRegularFileIdentity(
        path=opened.path,
        size=opened.stat.st_size,
        sha256=digest,
        _stat_identity=_stat_identity(opened.stat),
        _content_change_time_ns=opened.content_change_time_ns,
    )
    if owned_snapshot_identity is None:
        raise RuntimeError("stable snapshot lost its destination ownership identity")
    return StableRegularFileSnapshot(
        source=source_identity,
        snapshot=snapshot_identity,
        prefix=bytes(prefix),
    )


def verify_stable_regular_file_identity(
    identity: StableRegularFileIdentity,
    *,
    label: str,
) -> None:
    """Fail unless a file still has the captured handle and mutation identity."""

    _path, file_stat, change_time_ns, _digest = _stable_regular_file_snapshot(
        identity.path,
        label=label,
        hash_content=False,
    )
    if (
        _stat_identity(file_stat) != identity._stat_identity
        or change_time_ns != identity._content_change_time_ns
    ):
        raise ValueError(f"{label} changed since identity capture: {identity.path}")


def read_stable_regular_file(
    identity: StableRegularFileIdentity, *, label: str
) -> bytes:
    """Read attested bytes without rehashing or retaining a second content cache."""

    with open_stable_regular_file(identity.path, label=label) as opened:
        if (
            _stat_identity(opened.stat) != identity._stat_identity
            or opened.content_change_time_ns != identity._content_change_time_ns
        ):
            raise StableRegularFileChangedError(
                f"{label} changed since identity capture: {identity.path}"
            )
        data = opened.stream.read()
        if len(data) != identity.size:
            raise StableRegularFileChangedError(
                f"{label} ended before its captured size: {identity.path}"
            )
    return data


@contextmanager
def stable_executable_probe(
    path: Path,
    *,
    label: str,
    identity: StableRegularFileIdentity | None = None,
) -> Iterator[tuple[Path, StableRegularFileIdentity]]:
    """Bind a probe to a cold-captured or already-attested executable generation."""
    entrypoint, resolved = _executable_paths(path, label=label)
    before_entry = _stat_identity(entrypoint.lstat())
    if identity is None:
        identity = stable_regular_file_identity(resolved, label=label)
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
) -> Iterator[tuple[Path, StableRegularFileIdentity]]:
    """Bind native executable admission and a probe to one captured generation."""
    with stable_executable_probe(path, label=label) as (entrypoint, identity):
        with open_stable_regular_file(identity.path, label=label) as opened:
            header = opened.stream.read(4)
        if not _native_executable_header(header):
            raise ValueError(
                f"{label} must be a native executable, not a script or delegating wrapper: "
                f"{identity.path}"
            )
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
                    env=dict(environment),
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
