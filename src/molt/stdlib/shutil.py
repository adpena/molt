"""Intrinsic-backed shutil for Molt -- all operations delegated to Rust."""

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


def copyfile(src: str, dst: str) -> str:
    """Copy data from src to dst. Returns the destination path."""
    out = _MOLT_SHUTIL_COPYFILE(src, dst)
    if not isinstance(out, str):
        raise RuntimeError("shutil.copyfile intrinsic returned invalid value")
    return out


def which(cmd: str, mode: int | None = None, path: str | None = None) -> str | None:
    """Return the path to an executable which would be run for the given cmd."""
    del mode
    out = _MOLT_SHUTIL_WHICH(cmd, path)
    if out is None:
        return None
    if not isinstance(out, str):
        raise RuntimeError("shutil.which intrinsic returned invalid value")
    return out


def rmtree(path: str, ignore_errors: bool = False, onerror=None) -> None:
    """Recursively delete a directory tree (via Rust intrinsic)."""
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


def disk_usage(path: str) -> tuple[int, int, int]:
    """Return disk usage statistics (total, used, free) via Rust intrinsic."""
    raw = _MOLT_SHUTIL_DISK_USAGE(path)
    total, used, free = int(raw[0]), int(raw[1]), int(raw[2])
    return (total, used, free)


def get_terminal_size(
    fallback: tuple[int, int] = (80, 24),
) -> tuple[int, int]:
    """Get terminal window size (columns, lines) via Rust intrinsic."""
    raw = _MOLT_SHUTIL_GET_TERMINAL_SIZE(list(fallback))
    columns, lines = int(raw[0]), int(raw[1])
    return (columns, lines)


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
) -> None:
    """Unpack an archive (via Rust intrinsic)."""
    _MOLT_SHUTIL_UNPACK_ARCHIVE(filename, extract_dir)


globals().pop("_require_intrinsic", None)
