"""Intrinsic-backed shutil subset for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

__all__ = [
    "chown",
    "copy",
    "copy2",
    "copyfile",
    "copytree",
    "disk_usage",
    "get_terminal_size",
    "make_archive",
    "move",
    "rmtree",
    "unpack_archive",
    "which",
]


_MOLT_SHUTIL_COPYFILE = _require_intrinsic("molt_shutil_copyfile", globals())
_MOLT_SHUTIL_WHICH = _require_intrinsic("molt_shutil_which", globals())
_MOLT_SHUTIL_RMTREE = _require_intrinsic("molt_shutil_rmtree", globals())
_MOLT_SHUTIL_CHOWN = _require_intrinsic("molt_shutil_chown", globals())
_MOLT_SHUTIL_COPY = _require_intrinsic("molt_shutil_copy", globals())
_MOLT_SHUTIL_COPY2 = _require_intrinsic("molt_shutil_copy2", globals())
_MOLT_SHUTIL_COPYTREE = _require_intrinsic("molt_shutil_copytree", globals())
_MOLT_SHUTIL_DISK_USAGE = _require_intrinsic("molt_shutil_disk_usage", globals())
_MOLT_SHUTIL_GET_TERMINAL_SIZE = _require_intrinsic(
    "molt_shutil_get_terminal_size", globals()
)
_MOLT_SHUTIL_MAKE_ARCHIVE = _require_intrinsic("molt_shutil_make_archive", globals())
_MOLT_SHUTIL_MOVE = _require_intrinsic("molt_shutil_move", globals())
_MOLT_SHUTIL_UNPACK_ARCHIVE = _require_intrinsic(
    "molt_shutil_unpack_archive", globals()
)


def copyfile(src: str, dst: str) -> str:
    out = _MOLT_SHUTIL_COPYFILE(src, dst)
    if not isinstance(out, str):
        raise RuntimeError("shutil.copyfile intrinsic returned invalid value")
    return out


def which(cmd: str, mode: int | None = None, path: str | None = None) -> str | None:
    del mode
    out = _MOLT_SHUTIL_WHICH(cmd, path)
    if out is None:
        return None
    if not isinstance(out, str):
        raise RuntimeError("shutil.which intrinsic returned invalid value")
    return out


def rmtree(path: str, ignore_errors: bool = False) -> None:
    if ignore_errors:
        try:
            _MOLT_SHUTIL_RMTREE(path)
        except OSError:
            return
        return
    _MOLT_SHUTIL_RMTREE(path)


def chown(
    path: str,
    user: str | int | None = None,
    group: str | int | None = None,
) -> None:
    """Change owner and group of a file."""
    _MOLT_SHUTIL_CHOWN(path, user, group)


def copy(src: str, dst: str) -> str:
    """Copy data and permissions. Returns the destination path."""
    out = _MOLT_SHUTIL_COPY(src, dst)
    return str(out)


def copy2(src: str, dst: str) -> str:
    """Copy data and all metadata. Returns the destination path."""
    out = _MOLT_SHUTIL_COPY2(src, dst)
    return str(out)


def copytree(
    src: str,
    dst: str,
    dirs_exist_ok: bool = False,
) -> str:
    """Recursively copy an entire directory tree. Returns the destination path."""
    out = _MOLT_SHUTIL_COPYTREE(src, dst, bool(dirs_exist_ok))
    return str(out)


def disk_usage(path: str) -> tuple[int, int, int]:
    """Return disk usage statistics about the given path as a named tuple."""
    raw = _MOLT_SHUTIL_DISK_USAGE(path)
    # Intrinsic returns a (total, used, free) sequence.
    total, used, free = int(raw[0]), int(raw[1]), int(raw[2])
    return (total, used, free)


def get_terminal_size(
    fallback: tuple[int, int] = (80, 24),
) -> tuple[int, int]:
    """Get the size of the terminal window."""
    raw = _MOLT_SHUTIL_GET_TERMINAL_SIZE(list(fallback))
    columns, lines = int(raw[0]), int(raw[1])
    return (columns, lines)


def make_archive(
    base_name: str,
    format: str,
    root_dir: str | None = None,
) -> str:
    """Create an archive file and return its name."""
    out = _MOLT_SHUTIL_MAKE_ARCHIVE(base_name, format, root_dir)
    return str(out)


def move(src: str, dst: str) -> str:
    """Recursively move a file or directory. Returns the destination path."""
    out = _MOLT_SHUTIL_MOVE(src, dst)
    return str(out)


def unpack_archive(
    filename: str,
    extract_dir: str | None = None,
) -> None:
    """Unpack an archive. *extract_dir* defaults to the current directory."""
    _MOLT_SHUTIL_UNPACK_ARCHIVE(filename, extract_dir)
