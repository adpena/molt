"""Intrinsic-backed shutil for Molt -- all operations delegated to Rust."""

from __future__ import annotations

import os as _os
from typing import IO as _IO

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_REQUIRE_OPERATION = _require_intrinsic("molt_capabilities_require_operation")
_MOLT_NEED_CURRENT_DIRECTORY = _require_intrinsic(
    "molt_os_need_current_directory_for_exe_path"
)

__all__ = [
    "COPY_BUFSIZE",
    "Error",
    "ReadError",
    "RegistryError",
    "SameFileError",
    "SpecialFileError",
    "chown",
    "copy",
    "copy2",
    "copyfile",
    "copyfileobj",
    "copymode",
    "copystat",
    "copytree",
    "disk_usage",
    "get_archive_formats",
    "get_terminal_size",
    "get_unpack_formats",
    "ignore_patterns",
    "make_archive",
    "move",
    "register_archive_format",
    "register_unpack_format",
    "rmtree",
    "unpack_archive",
    "unregister_archive_format",
    "unregister_unpack_format",
    "which",
]

# ── Constants ────────────────────────────────────────────────────────────────

COPY_BUFSIZE = 1024 * 1024  # 1 MiB, matches CPython


# ── Exceptions ───────────────────────────────────────────────────────────────


class Error(OSError):
    pass


class SameFileError(Error):
    pass


class SpecialFileError(OSError):
    pass


class ReadError(OSError):
    pass


class RegistryError(Exception):
    pass


# ── Intrinsic bindings ────────────────────────────────────────────────────────

_MOLT_SHUTIL_COPYFILE = _require_intrinsic("molt_shutil_copyfile")
_MOLT_SHUTIL_WHICH = _require_intrinsic("molt_shutil_which")
_MOLT_SHUTIL_RMTREE = _require_intrinsic("molt_shutil_rmtree")
_MOLT_SHUTIL_CHOWN = _require_intrinsic("molt_shutil_chown")
_MOLT_SHUTIL_COPY = _require_intrinsic("molt_shutil_copy")
_MOLT_SHUTIL_COPY2 = _require_intrinsic("molt_shutil_copy2")
_MOLT_SHUTIL_COPYTREE = _require_intrinsic("molt_shutil_copytree")
_MOLT_SHUTIL_DISK_USAGE = _require_intrinsic("molt_shutil_disk_usage")
_MOLT_SHUTIL_GET_TERMINAL_SIZE = _require_intrinsic("molt_shutil_get_terminal_size")
_MOLT_SHUTIL_MAKE_ARCHIVE = _require_intrinsic("molt_shutil_make_archive")
_MOLT_SHUTIL_MOVE = _require_intrinsic("molt_shutil_move")
_MOLT_SHUTIL_UNPACK_ARCHIVE = _require_intrinsic("molt_shutil_unpack_archive")
_MOLT_SHUTIL_COPYSTAT = _require_intrinsic("molt_shutil_copystat")
_MOLT_SHUTIL_COPYMODE = _require_intrinsic("molt_shutil_copymode")


# ── Archive format registry ───────────────────────────────────────────────────

_ARCHIVE_FORMATS: dict[str, tuple] = {}
_UNPACK_FORMATS: dict[str, tuple] = {}


def register_archive_format(
    name: str,
    function,
    extra_args=None,
    description: str = "",
) -> None:
    """Register an archive format.  *name* is the name of the format."""
    if not callable(function):
        raise TypeError("The callable argument must be callable")
    if extra_args is None:
        extra_args = []
    _ARCHIVE_FORMATS[name] = (function, extra_args, description)


def unregister_archive_format(name: str) -> None:
    """Remove the archive format *name* from the list of supported formats."""
    del _ARCHIVE_FORMATS[name]


def get_archive_formats() -> list[tuple[str, str]]:
    """Return a list of supported archive formats as (name, description) pairs."""
    return [(name, info[2]) for name, info in _ARCHIVE_FORMATS.items()]


def register_unpack_format(
    name: str,
    extensions: list[str],
    function,
    extra_args=None,
    description: str = "",
) -> None:
    """Register an unpack format."""
    if not callable(function):
        raise TypeError("The callable argument must be callable")
    if extra_args is None:
        extra_args = []
    _UNPACK_FORMATS[name] = (extensions, function, extra_args, description)


def unregister_unpack_format(name: str) -> None:
    """Remove the unpack format *name* from the list of supported formats."""
    del _UNPACK_FORMATS[name]


def get_unpack_formats() -> list[tuple[str, list[str], str]]:
    """Return a list of supported unpack formats as (name, extensions, description) triples."""
    return [(name, info[0], info[3]) for name, info in _UNPACK_FORMATS.items()]


# ── Core file operations ──────────────────────────────────────────────────────


def copyfileobj(fsrc: _IO, fdst: _IO, length: int = COPY_BUFSIZE) -> None:
    """Copy data from file-like object fsrc to file-like object fdst."""
    while True:
        buf = fsrc.read(length)
        if not buf:
            break
        fdst.write(buf)


def copyfile(src: str, dst: str, *, follow_symlinks: bool = True) -> str:
    """Copy data from src to dst. Returns the destination path."""
    out = _MOLT_SHUTIL_COPYFILE(src, dst)
    if not isinstance(out, str):
        raise RuntimeError("shutil.copyfile intrinsic returned invalid value")
    return out


def copymode(src: str, dst: str, *, follow_symlinks: bool = True) -> None:
    """Copy permission bits from src to dst (via Rust intrinsic)."""
    _MOLT_SHUTIL_COPYMODE(src, dst, bool(follow_symlinks))


def copystat(src: str, dst: str, *, follow_symlinks: bool = True) -> None:
    """Copy file metadata from src to dst (via Rust intrinsic)."""
    _MOLT_SHUTIL_COPYSTAT(src, dst, bool(follow_symlinks))


def _access_check(filename, mode):
    return (
        _os.path.exists(filename)
        and _os.access(filename, mode)
        and not _os.path.isdir(filename)
    )


def which(cmd, mode=_os.F_OK | _os.X_OK, path=None):
    """Return the first accessible command from the selected search inputs."""
    # Inspect builtin storage without dispatching subclass callbacks, so the
    # complete selected grant set precedes fspath/codecs/mapping callbacks.
    use_bytes = isinstance(cmd, bytes)
    raw = bytes if use_bytes else str
    slash = b"/" if use_bytes else "/"
    backslash = b"\\" if use_bytes else "\\"
    has_directory = isinstance(cmd, (str, bytes)) and (
        raw.rfind(cmd, slash) >= 0
        or (
            _os.name == "nt"
            and (
                raw.rfind(cmd, backslash) >= 0
                or raw.find(cmd, b":" if use_bytes else ":") >= 0
            )
        )
    )
    ambient = _os.name == "nt" or (path is None and not has_directory)
    _MOLT_REQUIRE_OPERATION("shutil.which" if ambient else "shutil.which.explicit_path")
    cmd = _os.fspath(cmd)
    if isinstance(cmd, bytes):
        use_bytes = True
    if _os.name == "nt":
        drive, root, tail = _os._path_splitroot(cmd, False)
        separators = b"\\/" if use_bytes else "\\/"
        index = len(tail)
        while index and tail[index - 1] not in separators:
            index -= 1
        head, cmd = tail[:index], tail[index:]
        dirname = drive + root + head.rstrip(separators)
    else:
        separator = b"/" if use_bytes else "/"
        index = cmd.rfind(separator) + 1
        dirname, cmd = cmd[:index], cmd[index:]
        if dirname and dirname != separator * len(dirname):
            dirname = dirname.rstrip(separator)
    if dirname:
        directories = [dirname]
    else:
        if path is None:
            path = _os.environ.get("PATH")
            if path is None:
                path = ".;C:\\bin" if _os.name == "nt" else "/bin:/usr/bin"
        if not path:
            return None
        path = _os.fsencode(path) if use_bytes else _os.fsdecode(path)
        directories = path.split(
            _os.fsencode(_os.pathsep) if use_bytes else _os.pathsep
        )
        if _os.name == "nt" and (
            not mode & _os.X_OK or _MOLT_NEED_CURRENT_DIRECTORY(_os.fsdecode(cmd))
        ):
            directories.insert(0, _os.fsencode(_os.curdir) if use_bytes else _os.curdir)
    if _os.name == "nt":
        source = _os.environ.get("PATHEXT") or ".COM;.EXE;.BAT;.CMD"
        extensions = [ext.rstrip(".") for ext in source.split(_os.pathsep) if ext]
        if use_bytes:
            extensions = [_os.fsencode(ext) for ext in extensions]
        files = [cmd + ext for ext in extensions]
        if not mode & _os.X_OK or any(
            cmd.upper().endswith(ext.upper()) for ext in extensions
        ):
            files.insert(0, cmd)
    else:
        files = [cmd]
    seen = set()
    unique = []
    for directory in directories:
        normalized = (
            directory.lower().replace(
                b"/" if use_bytes else "/", b"\\" if use_bytes else "\\"
            )
            if _os.name == "nt"
            else directory
        )
        if normalized not in seen:
            seen.add(normalized)
            unique.append(directory)
    return _MOLT_SHUTIL_WHICH(files, unique, mode)


def rmtree(
    path: str,
    ignore_errors: bool = False,
    onerror=None,
    *,
    onexc=None,
    dir_fd=None,
) -> None:
    """Recursively delete a directory tree (via Rust intrinsic)."""
    if ignore_errors:
        try:
            _MOLT_SHUTIL_RMTREE(path)
        except OSError:
            return
        return
    try:
        _MOLT_SHUTIL_RMTREE(path)
    except OSError as exc:
        # onexc is the Python 3.12+ replacement for onerror.
        if onexc is not None:
            onexc(type(exc), exc, None)
        elif onerror is not None:
            onerror(type(exc), exc, None)
        else:
            raise


def chown(
    path: str,
    user: str | int | None = None,
    group: str | int | None = None,
) -> None:
    """Change owner and group of a file (via Rust intrinsic)."""
    _MOLT_SHUTIL_CHOWN(path, user, group)


def copy(src: str, dst: str, *, follow_symlinks: bool = True) -> str:
    """Copy data and permissions. Returns the destination path."""
    out = _MOLT_SHUTIL_COPY(src, dst)
    return str(out)


def copy2(src: str, dst: str, *, follow_symlinks: bool = True) -> str:
    """Copy data and all metadata. Returns the destination path."""
    out = _MOLT_SHUTIL_COPY2(src, dst)
    return str(out)


def copytree(
    src: str,
    dst: str,
    symlinks: bool = False,
    ignore=None,
    copy_function=None,
    ignore_dangling_symlinks: bool = False,
    dirs_exist_ok: bool = False,
) -> str:
    """Recursively copy an entire directory tree (via Rust intrinsic)."""
    out = _MOLT_SHUTIL_COPYTREE(src, dst, bool(dirs_exist_ok))
    return str(out)


def ignore_patterns(*patterns: str):
    """Factory function for use with copytree()'s ignore parameter.

    Returns a callable that receives a directory path and a list of
    directory contents, and returns a set of names that should be ignored.
    """
    import fnmatch as _fnmatch

    def _ignore_patterns(path: str, names: list[str]) -> set[str]:
        ignored: set[str] = set()
        for pattern in patterns:
            ignored.update(_fnmatch.filter(names, pattern))
        return ignored

    return _ignore_patterns


def disk_usage(path: str) -> tuple[int, int, int]:
    """Return disk usage statistics (total, used, free) via Rust intrinsic."""
    raw = _MOLT_SHUTIL_DISK_USAGE(path)
    total, used, free = int(raw[0]), int(raw[1]), int(raw[2])
    return (total, used, free)


def get_terminal_size(
    fallback: tuple[int, int] = (80, 24),
) -> _os.terminal_size:
    """Get terminal window size (columns, lines) via Rust intrinsic.

    Returns ``os.terminal_size``, as CPython does: callers such as argparse's
    ``HelpFormatter`` read ``.columns``.
    """
    raw = _MOLT_SHUTIL_GET_TERMINAL_SIZE(list(fallback))
    return _os.terminal_size((int(raw[0]), int(raw[1])))


def make_archive(
    base_name: str,
    format: str,
    root_dir: str | None = None,
    base_dir: str | None = None,
    verbose: int = 0,
    dry_run: int = 0,
    owner: str | None = None,
    group: str | None = None,
    logger: object = None,
) -> str:
    """Create an archive file and return its name (via Rust intrinsic)."""
    out = _MOLT_SHUTIL_MAKE_ARCHIVE(base_name, format, root_dir)
    return str(out)


def move(src: str, dst: str, copy_function=None) -> str:
    """Recursively move a file or directory (via Rust intrinsic)."""
    out = _MOLT_SHUTIL_MOVE(src, dst)
    return str(out)


def unpack_archive(
    filename: str,
    extract_dir: str | None = None,
    format: str | None = None,
    *,
    filter=None,
) -> None:
    """Unpack an archive (via Rust intrinsic)."""
    _MOLT_SHUTIL_UNPACK_ARCHIVE(filename, extract_dir)


globals().pop("_require_intrinsic", None)
