"""Intrinsic-backed stat constants/helpers for Molt."""

from __future__ import annotations

import sys

from _intrinsics import require_intrinsic as _require_intrinsic

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
_MOLT_STAT_ISDOOR = _require_intrinsic("molt_stat_isdoor", globals())
_MOLT_STAT_ISPORT = _require_intrinsic("molt_stat_isport", globals())
_MOLT_STAT_ISWHT = _require_intrinsic("molt_stat_iswht", globals())
_MOLT_STAT_FILEMODE = _require_intrinsic("molt_stat_filemode", globals())

_constants = _MOLT_STAT_CONSTANTS()
if (
    not isinstance(_constants, tuple)
    or len(_constants) != 71
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
    S_IFDOOR,
    S_IFPORT,
    S_IFWHT,
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
    UF_NODUMP,
    UF_IMMUTABLE,
    UF_APPEND,
    UF_OPAQUE,
    UF_NOUNLINK,
    UF_COMPRESSED,
    UF_HIDDEN,
    SF_ARCHIVED,
    SF_IMMUTABLE,
    SF_APPEND,
    SF_NOUNLINK,
    SF_SNAPSHOT,
    _UF_SETTABLE,
    _UF_TRACKED,
    _UF_DATAVAULT,
    _SF_SETTABLE,
    _SF_RESTRICTED,
    _SF_FIRMLINK,
    _SF_DATALESS,
    _SF_SUPPORTED,
    _SF_SYNTHETIC,
    FILE_ATTRIBUTE_ARCHIVE,
    FILE_ATTRIBUTE_COMPRESSED,
    FILE_ATTRIBUTE_DEVICE,
    FILE_ATTRIBUTE_DIRECTORY,
    FILE_ATTRIBUTE_ENCRYPTED,
    FILE_ATTRIBUTE_HIDDEN,
    FILE_ATTRIBUTE_INTEGRITY_STREAM,
    FILE_ATTRIBUTE_NORMAL,
    FILE_ATTRIBUTE_NOT_CONTENT_INDEXED,
    FILE_ATTRIBUTE_NO_SCRUB_DATA,
    FILE_ATTRIBUTE_OFFLINE,
    FILE_ATTRIBUTE_READONLY,
    FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_ATTRIBUTE_SPARSE_FILE,
    FILE_ATTRIBUTE_SYSTEM,
    FILE_ATTRIBUTE_TEMPORARY,
    FILE_ATTRIBUTE_VIRTUAL,
) = _constants

_HAS_313_CONSTANTS = sys.version_info >= (3, 13)

if _HAS_313_CONSTANTS:
    UF_SETTABLE = _UF_SETTABLE
    UF_TRACKED = _UF_TRACKED
    UF_DATAVAULT = _UF_DATAVAULT
    SF_SETTABLE = _SF_SETTABLE
    SF_RESTRICTED = _SF_RESTRICTED
    SF_FIRMLINK = _SF_FIRMLINK
    SF_DATALESS = _SF_DATALESS
    SF_SUPPORTED = _SF_SUPPORTED
    SF_SYNTHETIC = _SF_SYNTHETIC
else:
    if any(
        value != 0
        for value in (
            _UF_SETTABLE,
            _UF_TRACKED,
            _UF_DATAVAULT,
            _SF_SETTABLE,
            _SF_RESTRICTED,
            _SF_FIRMLINK,
            _SF_DATALESS,
            _SF_SUPPORTED,
            _SF_SYNTHETIC,
        )
    ):
        raise RuntimeError("stat constants intrinsic returned unexpected 3.13+ payload")

S_ENFMT = S_ISGID
S_IREAD = S_IRUSR
S_IWRITE = S_IWUSR
S_IEXEC = S_IXUSR

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


def S_ISDOOR(mode: int) -> bool:
    return bool(_MOLT_STAT_ISDOOR(mode))


def S_ISPORT(mode: int) -> bool:
    return bool(_MOLT_STAT_ISPORT(mode))


def S_ISWHT(mode: int) -> bool:
    return bool(_MOLT_STAT_ISWHT(mode))


def filemode(mode: int) -> str:
    return _MOLT_STAT_FILEMODE(mode)
