"""Intrinsic-backed stat constants/helpers for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

__all__ = [
    "S_IFMT",
    "S_IMODE",
    "S_IFSOCK",
    "S_IFLNK",
    "S_IFREG",
    "S_IFBLK",
    "S_IFDIR",
    "S_IFCHR",
    "S_IFIFO",
    "S_ISUID",
    "S_ISGID",
    "S_ISVTX",
    "S_IRUSR",
    "S_IWUSR",
    "S_IXUSR",
    "S_IRGRP",
    "S_IWGRP",
    "S_IXGRP",
    "S_IROTH",
    "S_IWOTH",
    "S_IXOTH",
    "S_IRWXU",
    "S_IRWXG",
    "S_IRWXO",
    "S_ISDIR",
    "S_ISREG",
    "S_ISCHR",
    "S_ISBLK",
    "S_ISFIFO",
    "S_ISLNK",
    "S_ISSOCK",
    "ST_MODE",
    "ST_INO",
    "ST_DEV",
    "ST_NLINK",
    "ST_UID",
    "ST_GID",
    "ST_SIZE",
    "ST_ATIME",
    "ST_MTIME",
    "ST_CTIME",
]

_MOLT_STAT_CONSTANTS = _require_intrinsic("molt_stat_constants", globals())
_MOLT_STAT_IFMT = _require_intrinsic("molt_stat_ifmt", globals())
_MOLT_STAT_IMODE = _require_intrinsic("molt_stat_imode", globals())
_MOLT_STAT_ISDIR = _require_intrinsic("molt_stat_isdir", globals())
_MOLT_STAT_ISREG = _require_intrinsic("molt_stat_isreg", globals())
_MOLT_STAT_ISCHR = _require_intrinsic("molt_stat_ischr", globals())
_MOLT_STAT_ISBLK = _require_intrinsic("molt_stat_isblk", globals())
_MOLT_STAT_ISFIFO = _require_intrinsic("molt_stat_isfifo", globals())
_MOLT_STAT_ISLNK = _require_intrinsic("molt_stat_islnk", globals())
_MOLT_STAT_ISSOCK = _require_intrinsic("molt_stat_issock", globals())

_constants = _MOLT_STAT_CONSTANTS()
if (
    not isinstance(_constants, tuple)
    or len(_constants) != 30
    or not all(isinstance(v, int) for v in _constants)
):
    raise RuntimeError("stat constants intrinsic returned invalid value")

(
    _S_IFMT_MASK,
    S_IFSOCK,
    S_IFLNK,
    S_IFREG,
    S_IFBLK,
    S_IFDIR,
    S_IFCHR,
    S_IFIFO,
    S_ISUID,
    S_ISGID,
    S_ISVTX,
    S_IRUSR,
    S_IWUSR,
    S_IXUSR,
    S_IRGRP,
    S_IWGRP,
    S_IXGRP,
    S_IROTH,
    S_IWOTH,
    S_IXOTH,
    ST_MODE,
    ST_INO,
    ST_DEV,
    ST_NLINK,
    ST_UID,
    ST_GID,
    ST_SIZE,
    ST_ATIME,
    ST_MTIME,
    ST_CTIME,
) = _constants

S_IRWXU = S_IRUSR | S_IWUSR | S_IXUSR
S_IRWXG = S_IRGRP | S_IWGRP | S_IXGRP
S_IRWXO = S_IROTH | S_IWOTH | S_IXOTH


def S_IFMT(mode: int) -> int:
    return int(_MOLT_STAT_IFMT(mode))


def S_IMODE(mode: int) -> int:
    return int(_MOLT_STAT_IMODE(mode))


def S_ISDIR(mode: int) -> bool:
    return bool(_MOLT_STAT_ISDIR(mode))


def S_ISREG(mode: int) -> bool:
    return bool(_MOLT_STAT_ISREG(mode))


def S_ISCHR(mode: int) -> bool:
    return bool(_MOLT_STAT_ISCHR(mode))


def S_ISBLK(mode: int) -> bool:
    return bool(_MOLT_STAT_ISBLK(mode))


def S_ISFIFO(mode: int) -> bool:
    return bool(_MOLT_STAT_ISFIFO(mode))


def S_ISLNK(mode: int) -> bool:
    return bool(_MOLT_STAT_ISLNK(mode))


def S_ISSOCK(mode: int) -> bool:
    return bool(_MOLT_STAT_ISSOCK(mode))
