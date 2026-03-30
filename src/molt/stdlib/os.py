"""Minimal os shim for Molt."""

from __future__ import annotations

import abc as _abc
import sys as _sys
import types as _types

from _intrinsics import require_intrinsic as _require_intrinsic

TYPE_CHECKING = False

if TYPE_CHECKING:
    from typing import Any, Iterator
else:

    class _TypingAlias:
        __slots__ = ()

        def __getitem__(self, _item):
            return self

    Any = object
    Iterator = _TypingAlias()
    ItemsView = _TypingAlias()
    KeysView = _TypingAlias()
    ValuesView = _TypingAlias()


_MOLT_ENV_GET = _require_intrinsic("molt_env_get")
_MOLT_ENV_SNAPSHOT = _require_intrinsic("molt_env_snapshot")
_MOLT_ENV_SET = _require_intrinsic("molt_env_set")
_MOLT_ENV_UNSET = _require_intrinsic("molt_env_unset")
_MOLT_ENV_LEN = _require_intrinsic("molt_env_len")
_MOLT_ENV_CONTAINS = _require_intrinsic("molt_env_contains")
_MOLT_ENV_POPITEM = _require_intrinsic("molt_env_popitem")
_MOLT_ENV_CLEAR = _require_intrinsic("molt_env_clear")
_MOLT_ENV_PUTENV = _require_intrinsic("molt_env_putenv")
_MOLT_ENV_UNSETENV = _require_intrinsic("molt_env_unsetenv")
_MOLT_OS_NAME = _require_intrinsic("molt_os_name")
_MOLT_PATH_EXISTS = _require_intrinsic("molt_path_exists")
_MOLT_PATH_LISTDIR = _require_intrinsic("molt_path_listdir")
_MOLT_PATH_MKDIR = _require_intrinsic("molt_path_mkdir")
_MOLT_PATH_UNLINK = _require_intrinsic("molt_path_unlink")
_MOLT_PATH_RMDIR = _require_intrinsic("molt_path_rmdir")
_MOLT_PATH_CHMOD = _require_intrinsic("molt_path_chmod")
_MOLT_GETCWD = _require_intrinsic("molt_getcwd")
_MOLT_GETPID = _require_intrinsic("molt_getpid")
_MOLT_PATH_JOIN_MANY = _require_intrinsic("molt_path_join_many")
_MOLT_PATH_ISABS = _require_intrinsic("molt_path_isabs")
_MOLT_PATH_DIRNAME = _require_intrinsic("molt_path_dirname")
_MOLT_PATH_BASENAME = _require_intrinsic("molt_path_basename")
_MOLT_PATH_SPLIT = _require_intrinsic("molt_path_split")
_MOLT_PATH_SPLITEXT = _require_intrinsic("molt_path_splitext")
_MOLT_PATH_NORMPATH = _require_intrinsic("molt_path_normpath")
_MOLT_PATH_ABSPATH = _require_intrinsic("molt_path_abspath")
_MOLT_PATH_RELPATH = _require_intrinsic("molt_path_relpath")
_MOLT_PATH_EXPANDUSER = _require_intrinsic("molt_path_expanduser")
_MOLT_PATH_EXPANDVARS_ENV = _require_intrinsic("molt_path_expandvars_env")
_MOLT_PATH_MAKEDIRS = _require_intrinsic("molt_path_makedirs")
_MOLT_PATH_ISDIR = _require_intrinsic("molt_path_isdir")
_MOLT_PATH_ISFILE = _require_intrinsic("molt_path_isfile")
_MOLT_PATH_ISLINK = _require_intrinsic("molt_path_islink")
_MOLT_PATH_READLINK = _require_intrinsic("molt_path_readlink")
_MOLT_PATH_SYMLINK = _require_intrinsic("molt_path_symlink")
_MOLT_OS_OPEN = _require_intrinsic("molt_os_open")
_MOLT_OS_CLOSE = _require_intrinsic("molt_os_close")
_MOLT_OS_READ = _require_intrinsic("molt_os_read")
_MOLT_OS_WRITE = _require_intrinsic("molt_os_write")
_MOLT_OS_PIPE = _require_intrinsic("molt_os_pipe")
_MOLT_OS_DUP = _require_intrinsic("molt_os_dup")
_MOLT_OS_DUP2 = _require_intrinsic("molt_os_dup2")
_MOLT_OS_GET_INHERITABLE = _require_intrinsic("molt_os_get_inheritable")
_MOLT_OS_SET_INHERITABLE = _require_intrinsic("molt_os_set_inheritable")
_MOLT_OS_URANDOM = _require_intrinsic("molt_os_urandom")
_MOLT_OS_FSENCODE = _require_intrinsic("molt_os_fsencode")
_MOLT_OS_ACCESS = _require_intrinsic("molt_os_access")
_MOLT_OS_CHDIR = _require_intrinsic("molt_os_chdir")
_MOLT_OS_CPU_COUNT = _require_intrinsic("molt_os_cpu_count")
_MOLT_OS_LINK = _require_intrinsic("molt_os_link")
_MOLT_OS_ENVIRON = _require_intrinsic("molt_os_environ")
_MOLT_OS_MAKEDIRS = _require_intrinsic("molt_os_makedirs")
_MOLT_OS_PATH_JOIN = _require_intrinsic("molt_os_path_join")
_MOLT_OS_PATH_EXISTS = _require_intrinsic("molt_os_path_exists")
_MOLT_OS_PATH_ISFILE = _require_intrinsic("molt_os_path_isfile")
_MOLT_OS_PATH_ISDIR = _require_intrinsic("molt_os_path_isdir")
_MOLT_OS_TRUNCATE = _require_intrinsic("molt_os_truncate")
_MOLT_OS_UMASK = _require_intrinsic("molt_os_umask")
_MOLT_OS_UNAME = _require_intrinsic("molt_os_uname")
_MOLT_OS_GETPPID = _require_intrinsic("molt_os_getppid")
_MOLT_OS_GETUID = _require_intrinsic("molt_os_getuid")
_MOLT_OS_GETGID = _require_intrinsic("molt_os_getgid")
_MOLT_OS_GETEUID = _require_intrinsic("molt_os_geteuid")
_MOLT_OS_GETEGID = _require_intrinsic("molt_os_getegid")
_MOLT_OS_GETLOGIN = _require_intrinsic("molt_os_getlogin")
_MOLT_OS_GETLOADAVG = _require_intrinsic("molt_os_getloadavg")
_MOLT_OS_REMOVEDIRS = _require_intrinsic("molt_os_removedirs")
_MOLT_OS_DEVNULL = _require_intrinsic("molt_os_devnull")
_MOLT_OS_GET_TERMINAL_SIZE = _require_intrinsic("molt_os_get_terminal_size")
_MOLT_OS_WALK = _require_intrinsic("molt_os_walk")
_MOLT_OS_SCANDIR = _require_intrinsic("molt_os_scandir")
_MOLT_OS_LSEEK = _require_intrinsic("molt_os_lseek")
_MOLT_OS_FTRUNCATE = _require_intrinsic("molt_os_ftruncate")
_MOLT_OS_ISATTY = _require_intrinsic("molt_os_isatty")
_MOLT_OS_KILL = _require_intrinsic("molt_os_kill")
_MOLT_OS_WAITPID = _require_intrinsic("molt_os_waitpid")
_MOLT_OS_GETPGRP = _require_intrinsic("molt_os_getpgrp")
_MOLT_OS_SETPGRP = _require_intrinsic("molt_os_setpgrp")
_MOLT_OS_SETSID = _require_intrinsic("molt_os_setsid")
_MOLT_OS_SYSCONF = _require_intrinsic("molt_os_sysconf")
_MOLT_OS_SYSCONF_NAMES = _require_intrinsic("molt_os_sysconf_names")
_MOLT_OS_PATH_REALPATH = _require_intrinsic("molt_os_path_realpath")
_MOLT_OS_UTIME = _require_intrinsic("molt_os_utime")
_MOLT_OS_SENDFILE = _require_intrinsic("molt_os_sendfile")
_MOLT_OS_PATH_COMMONPATH = _require_intrinsic("molt_os_path_commonpath")
_MOLT_OS_PATH_COMMONPREFIX = _require_intrinsic("molt_os_path_commonprefix")
_MOLT_OS_PATH_GETATIME = _require_intrinsic("molt_os_path_getatime")
_MOLT_OS_PATH_GETCTIME = _require_intrinsic("molt_os_path_getctime")
_MOLT_OS_PATH_GETMTIME = _require_intrinsic("molt_os_path_getmtime")
_MOLT_OS_PATH_GETSIZE = _require_intrinsic("molt_os_path_getsize")
_MOLT_OS_PATH_SAMEFILE = _require_intrinsic("molt_os_path_samefile")
_MOLT_OS_FDOPEN = _require_intrinsic("molt_os_fdopen")
_MOLT_OS_SEP = _require_intrinsic("molt_os_sep")
_MOLT_OS_ALTSEP = _require_intrinsic("molt_os_altsep")
_MOLT_OS_CURDIR = _require_intrinsic("molt_os_curdir")
_MOLT_OS_PARDIR = _require_intrinsic("molt_os_pardir")
_MOLT_OS_EXTSEP = _require_intrinsic("molt_os_extsep")
_MOLT_OS_PATHSEP = _require_intrinsic("molt_os_pathsep")
_MOLT_OS_LINESEP = _require_intrinsic("molt_os_linesep")
_MOLT_OS_LISTDIR_V2 = _require_intrinsic("molt_os_listdir")
_MOLT_OS_GETCWD_V2 = _require_intrinsic("molt_os_getcwd")
_MOLT_OS_GETPID_V2 = _require_intrinsic("molt_os_getpid")
_MOLT_OS_MKDIR_V2 = _require_intrinsic("molt_os_mkdir")
_MOLT_OS_RMDIR_V2 = _require_intrinsic("molt_os_rmdir")
_MOLT_OS_CHMOD_V2 = _require_intrinsic("molt_os_chmod")
_MOLT_OS_SYMLINK_V2 = _require_intrinsic("molt_os_symlink")
_MOLT_OS_READLINK_V2 = _require_intrinsic("molt_os_readlink")
_MOLT_PATH_EXPANDVARS = _require_intrinsic("molt_path_expandvars")
_MOLT_CAP_REQUIRE = _require_intrinsic("molt_capabilities_require")
_MOLT_OS_WIFEXITED = _require_intrinsic("molt_os_wifexited")
_MOLT_OS_WEXITSTATUS = _require_intrinsic("molt_os_wexitstatus")
_MOLT_OS_WIFSIGNALED = _require_intrinsic("molt_os_wifsignaled")
_MOLT_OS_WTERMSIG = _require_intrinsic("molt_os_wtermsig")
_MOLT_OS_WIFSTOPPED = _require_intrinsic("molt_os_wifstopped")
_MOLT_OS_WSTOPSIG = _require_intrinsic("molt_os_wstopsig")
_MOLT_OS_FSPATH = _require_intrinsic("molt_os_fspath")


def _resolve_os_name() -> str:
    value = _MOLT_OS_NAME()
    if not isinstance(value, str):
        raise RuntimeError("os name intrinsic returned invalid value")
    return value


__all__ = [
    "name",
    "sep",
    "pathsep",
    "linesep",
    "curdir",
    "pardir",
    "extsep",
    "altsep",
    "devnull",
    "SEEK_SET",
    "SEEK_CUR",
    "SEEK_END",
    "F_OK",
    "R_OK",
    "W_OK",
    "X_OK",
    "O_RDONLY",
    "O_WRONLY",
    "O_RDWR",
    "O_APPEND",
    "O_CREAT",
    "O_EXCL",
    "O_TRUNC",
    "O_NONBLOCK",
    "O_CLOEXEC",
    "open",
    "getcwd",
    "getpid",
    "getppid",
    "getuid",
    "getgid",
    "geteuid",
    "getegid",
    "getlogin",
    "getenv",
    "putenv",
    "unsetenv",
    "urandom",
    "listdir",
    "scandir",
    "walk",
    "mkdir",
    "chmod",
    "makedirs",
    "removedirs",
    "rmdir",
    "read",
    "write",
    "close",
    "pipe",
    "dup",
    "dup2",
    "lseek",
    "ftruncate",
    "isatty",
    "get_inheritable",
    "set_inheritable",
    "stat_result",
    "stat",
    "lstat",
    "fstat",
    "rename",
    "replace",
    "unlink",
    "remove",
    "link",
    "readlink",
    "symlink",
    "access",
    "chdir",
    "cpu_count",
    "truncate",
    "umask",
    "uname",
    "uname_result",
    "getloadavg",
    "get_terminal_size",
    "terminal_size",
    "DirEntry",
    "environ",
    "kill",
    "waitpid",
    "getpgrp",
    "setpgrp",
    "setsid",
    "sysconf",
    "sysconf_names",
    "utime",
    "sendfile",
    "WNOHANG",
    "WUNTRACED",
    "WIFEXITED",
    "WEXITSTATUS",
    "WIFSIGNALED",
    "WTERMSIG",
    "WIFSTOPPED",
    "WSTOPSIG",
    "path",
    "PathLike",
    "fspath",
    "fsencode",
    "fdopen",
]

name = _resolve_os_name()
sep = _MOLT_OS_SEP()
altsep = _MOLT_OS_ALTSEP()
curdir = _MOLT_OS_CURDIR()
pardir = _MOLT_OS_PARDIR()
extsep = _MOLT_OS_EXTSEP()
pathsep = _MOLT_OS_PATHSEP()
linesep = _MOLT_OS_LINESEP()
SEEK_SET = 0
SEEK_CUR = 1
SEEK_END = 2

# waitpid options
WNOHANG = 1
WUNTRACED = 2


def WIFEXITED(status: int) -> bool:
    return bool(_MOLT_OS_WIFEXITED(status))


def WEXITSTATUS(status: int) -> int:
    return int(_MOLT_OS_WEXITSTATUS(status))


def WIFSIGNALED(status: int) -> bool:
    return bool(_MOLT_OS_WIFSIGNALED(status))


def WTERMSIG(status: int) -> int:
    return int(_MOLT_OS_WTERMSIG(status))


def WIFSTOPPED(status: int) -> bool:
    return bool(_MOLT_OS_WIFSTOPPED(status))


def WSTOPSIG(status: int) -> int:
    return int(_MOLT_OS_WSTOPSIG(status))


# access() mode constants
F_OK = 0
R_OK = 4
W_OK = 2
X_OK = 1

# POSIX file open flags — platform values sourced from libc at build time.
_MOLT_OS_OPEN_FLAGS = _require_intrinsic("molt_os_open_flags")
_open_flags = _MOLT_OS_OPEN_FLAGS()
O_RDONLY = _open_flags[0]
O_WRONLY = _open_flags[1]
O_RDWR = _open_flags[2]
O_APPEND = _open_flags[3]
O_CREAT = _open_flags[4]
O_TRUNC = _open_flags[5]
O_EXCL = _open_flags[6]
O_NONBLOCK = _open_flags[7]
O_CLOEXEC = _open_flags[8]
del _open_flags

# devnull — platform-appropriate null device path
_devnull_raw = _MOLT_OS_DEVNULL()
if not isinstance(_devnull_raw, str):
    raise RuntimeError("os devnull intrinsic returned invalid value")
devnull = _devnull_raw
del _devnull_raw

# sysconf_names — built from runtime intrinsic flat list
_sysconf_names_raw = _MOLT_OS_SYSCONF_NAMES()
sysconf_names: dict[str, int] = {}
if isinstance(_sysconf_names_raw, (list, tuple)):
    _i = 0
    while _i + 1 < len(_sysconf_names_raw):
        _k = _sysconf_names_raw[_i]
        _v = _sysconf_names_raw[_i + 1]
        if isinstance(_k, str) and isinstance(_v, int):
            sysconf_names[_k] = _v
        _i += 2
    del _i
del _sysconf_names_raw

_ENV_MISSING = object()

if TYPE_CHECKING:

    def molt_env_get(key: str, default: Any = None) -> Any:
        return default

    def _molt_getpid() -> int:
        return 0

    def _molt_getcwd() -> str:
        return curdir

    def _molt_path_exists(path: Any) -> bool:
        return False

    def _molt_path_unlink(path: Any) -> None:
        return None

    def _molt_path_rmdir(path: Any) -> None:
        return None


def _molt_env_get(key: str, default: Any = None) -> Any:
    return _MOLT_ENV_GET(key, default)


def _molt_env_snapshot() -> dict[str, str]:
    # Prefer the new consolidated intrinsic when available.
    if callable(_MOLT_OS_ENVIRON):
        raw = _MOLT_OS_ENVIRON()
        if isinstance(raw, dict):
            out: dict[str, str] = {}
            for key, value in raw.items():
                if not isinstance(key, str) or not isinstance(value, str):
                    raise RuntimeError("os env snapshot intrinsic returned invalid value")
                out[key] = value
            return out
        # The Rust side may return a flat list of [k, v, k, v, ...].
        if isinstance(raw, (list, tuple)):
            out2: dict[str, str] = {}
            it = iter(raw)
            for k in it:
                v = next(it)
                if not isinstance(k, str) or not isinstance(v, str):
                    raise RuntimeError("os env snapshot intrinsic returned invalid value")
                out2[k] = v
            return out2
    # Fallback to the legacy snapshot intrinsic.
    raw_legacy = _MOLT_ENV_SNAPSHOT()
    if not isinstance(raw_legacy, dict):
        raise RuntimeError("os env snapshot intrinsic returned invalid value")
    out3: dict[str, str] = {}
    for key, value in raw_legacy.items():
        if not isinstance(key, str) or not isinstance(value, str):
            raise RuntimeError("os env snapshot intrinsic returned invalid value")
        out3[key] = value
    return out3


def _molt_env_set(key: str, value: str) -> None:
    _MOLT_ENV_SET(key, value)


def _molt_env_unset(key: str) -> bool:
    return bool(_MOLT_ENV_UNSET(key))


def _molt_env_len() -> int:
    return int(_MOLT_ENV_LEN())


def _molt_env_contains(key: str) -> bool:
    return bool(_MOLT_ENV_CONTAINS(key))


def _molt_env_popitem() -> tuple[str, str]:
    value = _MOLT_ENV_POPITEM()
    if (
        isinstance(value, (tuple, list))
        and len(value) == 2
        and isinstance(value[0], str)
        and isinstance(value[1], str)
    ):
        return value[0], value[1]
    raise RuntimeError("os env popitem intrinsic returned invalid value")


def _molt_env_clear() -> None:
    _MOLT_ENV_CLEAR()


def _molt_env_putenv(key: str, value: str) -> None:
    _MOLT_ENV_PUTENV(key, value)


def _molt_env_unsetenv(key: str) -> None:
    _MOLT_ENV_UNSETENV(key)


def _require_cap(name: str) -> None:
    _MOLT_CAP_REQUIRE(name)
    return None


def _require_callable_intrinsic(value: Any, name: str):
    if not callable(value):
        raise RuntimeError(f"intrinsic unavailable: {name}")
    return value


def _require_os_intrinsic(name: str):
    return _require_callable_intrinsic(_require_intrinsic(name), name)


def _expect_str(value: Any, intrinsic: str) -> str:
    if not isinstance(value, str):
        raise RuntimeError(f"os {intrinsic} intrinsic returned invalid value")
    return value


def _expect_splitext(value: Any) -> tuple[str, str]:
    if (
        isinstance(value, (tuple, list))
        and len(value) == 2
        and isinstance(value[0], str)
        and isinstance(value[1], str)
    ):
        return value[0], value[1]
    raise RuntimeError("os splitext intrinsic returned invalid value")


def _expect_stat_result(value: Any, intrinsic: str) -> "stat_result":
    if not isinstance(value, (tuple, list)) or len(value) != 10:
        raise RuntimeError(f"os {intrinsic} intrinsic returned invalid value")
    head: list[int] = []
    tail: list[float] = []
    for index, field in enumerate(value):
        if isinstance(field, bool):
            raise RuntimeError(f"os {intrinsic} intrinsic returned invalid value")
        if index < 7:
            if not isinstance(field, int):
                raise RuntimeError(f"os {intrinsic} intrinsic returned invalid value")
            head.append(int(field))
        else:
            if not isinstance(field, (int, float)):
                raise RuntimeError(f"os {intrinsic} intrinsic returned invalid value")
            tail.append(float(field))
    return stat_result(tuple(head) + tuple(tail))


class stat_result(tuple):
    __slots__ = ()
    _SEQUENCE_FIELDS = (
        "st_mode",
        "st_ino",
        "st_dev",
        "st_nlink",
        "st_uid",
        "st_gid",
        "st_size",
        "st_atime",
        "st_mtime",
        "st_ctime",
    )
    n_fields = len(_SEQUENCE_FIELDS)
    n_sequence_fields = len(_SEQUENCE_FIELDS)
    n_unnamed_fields = 0

    def __new__(cls, values: tuple[int | float, ...]) -> "stat_result":
        if len(values) != len(cls._SEQUENCE_FIELDS):
            raise TypeError("os.stat_result() takes a 10-sequence")
        return tuple.__new__(cls, values)

    def __repr__(self) -> str:
        fields = ", ".join(
            f"{name}={self[index]!r}"
            for index, name in enumerate(self._SEQUENCE_FIELDS)
        )
        return f"os.stat_result({fields})"

    @property
    def st_mode(self) -> int:
        return int(self[0])

    @property
    def st_ino(self) -> int:
        return int(self[1])

    @property
    def st_dev(self) -> int:
        return int(self[2])

    @property
    def st_nlink(self) -> int:
        return int(self[3])

    @property
    def st_uid(self) -> int:
        return int(self[4])

    @property
    def st_gid(self) -> int:
        return int(self[5])

    @property
    def st_size(self) -> int:
        return int(self[6])

    @property
    def st_atime(self) -> float:
        return float(self[7])

    @property
    def st_mtime(self) -> float:
        return float(self[8])

    @property
    def st_ctime(self) -> float:
        return float(self[9])

    @property
    def st_atime_ns(self) -> int:
        return int(self.st_atime * 1_000_000_000)

    @property
    def st_mtime_ns(self) -> int:
        return int(self.st_mtime * 1_000_000_000)

    @property
    def st_ctime_ns(self) -> int:
        return int(self.st_ctime * 1_000_000_000)


class uname_result:
    """Result type for os.uname()."""

    __slots__ = ("sysname", "nodename", "release", "version", "machine")
    n_fields = 5
    n_sequence_fields = 5
    n_unnamed_fields = 0

    def __init__(
        self,
        sysname: str,
        nodename: str,
        release: str,
        version: str,
        machine: str,
    ) -> None:
        self.sysname = sysname
        self.nodename = nodename
        self.release = release
        self.version = version
        self.machine = machine

    def __repr__(self) -> str:
        return (
            f"posix.uname_result(sysname={self.sysname!r}, "
            f"nodename={self.nodename!r}, release={self.release!r}, "
            f"version={self.version!r}, machine={self.machine!r})"
        )

    def __iter__(self):
        yield self.sysname
        yield self.nodename
        yield self.release
        yield self.version
        yield self.machine

    def __getitem__(self, index: int) -> str:
        return (self.sysname, self.nodename, self.release, self.version, self.machine)[
            index
        ]

    def __len__(self) -> int:
        return 5

    def __eq__(self, other: Any) -> bool:
        if isinstance(other, uname_result):
            return (
                self.sysname == other.sysname
                and self.nodename == other.nodename
                and self.release == other.release
                and self.version == other.version
                and self.machine == other.machine
            )
        if isinstance(other, tuple):
            return tuple(self) == other
        return NotImplemented


class terminal_size(tuple):
    """Result type for os.get_terminal_size()."""

    __slots__ = ()

    def __new__(cls, values: tuple[int, int]) -> "terminal_size":
        if len(values) != 2:
            raise TypeError("terminal_size() takes a 2-sequence")
        return tuple.__new__(cls, (int(values[0]), int(values[1])))

    def __repr__(self) -> str:
        return f"os.terminal_size(columns={self.columns}, lines={self.lines})"

    @property
    def columns(self) -> int:
        return int(self[0])

    @property
    def lines(self) -> int:
        return int(self[1])


class DirEntry:
    """Entry yielded by os.scandir()."""

    __slots__ = ("name", "path", "_is_dir", "_is_file", "_is_symlink")

    def __init__(
        self,
        name: str,
        path: str,
        is_dir: bool,
        is_file: bool,
        is_symlink: bool,
    ) -> None:
        self.name = name
        self.path = path
        self._is_dir = is_dir
        self._is_file = is_file
        self._is_symlink = is_symlink

    def is_dir(self, *, follow_symlinks: bool = True) -> bool:
        return self._is_dir

    def is_file(self, *, follow_symlinks: bool = True) -> bool:
        return self._is_file

    def is_symlink(self) -> bool:
        return self._is_symlink

    def stat(self, *, follow_symlinks: bool = True) -> "stat_result":
        _require_cap("fs.read")
        if follow_symlinks:
            intrinsic = _require_os_intrinsic("molt_os_stat")
            return _expect_stat_result(intrinsic(self.path), "stat")
        intrinsic = _require_os_intrinsic("molt_os_lstat")
        return _expect_stat_result(intrinsic(self.path), "lstat")

    def inode(self) -> int:
        return self.stat(follow_symlinks=False).st_ino

    def __repr__(self) -> str:
        return f"<DirEntry {self.name!r}>"

    def __fspath__(self) -> str:
        return self.path


class _Environ:
    def _check_key(self, key: Any) -> str:
        if not isinstance(key, str):
            raise TypeError(f"str expected, not {type(key).__name__}")
        return key

    def _check_value(self, value: Any) -> str:
        if not isinstance(value, str):
            raise TypeError(f"str expected, not {type(value).__name__}")
        return value

    def __getitem__(self, key: str) -> str:
        _require_cap("env.read")
        key = self._check_key(key)
        value = _molt_env_get(key, _ENV_MISSING)
        if value is not _ENV_MISSING:
            return value
        raise KeyError(key)

    def __setitem__(self, key: str, value: str) -> None:
        _require_cap("env.write")
        key = self._check_key(key)
        value = self._check_value(value)
        _molt_env_set(key, value)

    def __delitem__(self, key: str) -> None:
        _require_cap("env.write")
        key = self._check_key(key)
        if not _molt_env_unset(key):
            raise KeyError(key)

    def __iter__(self) -> Iterator[str]:
        _require_cap("env.read")
        return iter(_molt_env_snapshot())

    def __len__(self) -> int:
        _require_cap("env.read")
        return _molt_env_len()

    def __contains__(self, key: object) -> bool:
        _require_cap("env.read")
        key = self._check_key(key)
        return _molt_env_contains(key)

    def __repr__(self) -> str:
        return "environ(" + repr(self.copy()) + ")"

    def copy(self) -> dict[str, str]:
        _require_cap("env.read")
        return _molt_env_snapshot()

    def get(self, key: str, default: Any = None) -> Any:
        _require_cap("env.read")
        key = self._check_key(key)
        value = _molt_env_get(key, _ENV_MISSING)
        if value is not _ENV_MISSING:
            return value
        return default

    def setdefault(self, key: str, default: Any = None) -> str:
        _require_cap("env.write")
        key = self._check_key(key)
        default = self._check_value(default)
        value = _molt_env_get(key, _ENV_MISSING)
        if value is not _ENV_MISSING:
            return value
        self[key] = default
        return default

    def update(self, other: Any = None, /, **kwargs: str) -> None:
        _require_cap("env.write")
        if other is not None:
            if hasattr(other, "items"):
                for key, value in other.items():
                    self[key] = value
            else:
                for key, value in other:
                    self[key] = value
        for key, value in kwargs.items():
            self[key] = value

    def pop(self, key: str, default: Any = _ENV_MISSING) -> Any:
        _require_cap("env.write")
        key = self._check_key(key)
        value = _molt_env_get(key, _ENV_MISSING)
        if value is not _ENV_MISSING:
            _molt_env_unset(key)
            return value
        if default is _ENV_MISSING:
            raise KeyError(key)
        return default

    def popitem(self) -> tuple[str, str]:
        _require_cap("env.write")
        return _molt_env_popitem()

    def clear(self) -> None:
        _require_cap("env.write")
        _molt_env_clear()

    def items(self) -> ItemsView[str, str]:
        _require_cap("env.read")
        return _molt_env_snapshot().items()

    def keys(self) -> KeysView[str]:
        _require_cap("env.read")
        return _molt_env_snapshot().keys()

    def values(self) -> ValuesView[str]:
        _require_cap("env.read")
        return _molt_env_snapshot().values()


class PathLike(_abc.ABC):
    __slots__ = ()

    @_abc.abstractmethod
    def __fspath__(self) -> str | bytes: ...


def fspath(path: Any) -> str | bytes:
    return _MOLT_OS_FSPATH(path)


def fsencode(filename: Any) -> bytes:
    intrinsic = _require_callable_intrinsic(_MOLT_OS_FSENCODE, "molt_os_fsencode")
    value = intrinsic(filename)
    if not isinstance(value, bytes):
        raise RuntimeError("os fsencode intrinsic returned invalid value")
    return value


class _Path:
    sep = sep
    pathsep = pathsep
    curdir = curdir
    pardir = pardir
    extsep = extsep
    altsep = altsep

    @staticmethod
    def join(
        first: Any,
        second: Any = None,
        third: Any = None,
        fourth: Any = None,
        fifth: Any = None,
        sixth: Any = None,
        seventh: Any = None,
        eighth: Any = None,
    ) -> Any:
        parts: list[Any] = []
        for arg in (first, second, third, fourth, fifth, sixth, seventh, eighth):
            if arg is not None:
                parts.append(arg)
        if not parts:
            return ""
        # Fast path: use the new 2-arg intrinsic for the common case.
        if len(parts) == 2 and callable(_MOLT_OS_PATH_JOIN):
            result = _MOLT_OS_PATH_JOIN(parts[0], parts[1])
            if isinstance(parts[0], bytes):
                return result
            return _expect_str(result, "os_path_join")
        result = _MOLT_PATH_JOIN_MANY(parts[0], tuple(parts[1:]))
        if isinstance(parts[0], bytes):
            return result
        return _expect_str(result, "path_join_many")

    @staticmethod
    def isabs(path: str) -> bool:
        return bool(_MOLT_PATH_ISABS(path))

    @staticmethod
    def dirname(path: str) -> str:
        return _expect_str(_MOLT_PATH_DIRNAME(path), "path_dirname")

    @staticmethod
    def basename(path: str) -> str:
        return _expect_str(_MOLT_PATH_BASENAME(path), "path_basename")

    @staticmethod
    def split(path: str) -> tuple[str, str]:
        value = _MOLT_PATH_SPLIT(path)
        if (
            isinstance(value, (tuple, list))
            and len(value) == 2
            and isinstance(value[0], str)
            and isinstance(value[1], str)
        ):
            return value[0], value[1]
        raise RuntimeError("os split intrinsic returned invalid value")

    @staticmethod
    def splitext(path: str) -> tuple[str, str]:
        return _expect_splitext(_MOLT_PATH_SPLITEXT(path))

    @staticmethod
    def normpath(path: str) -> str:
        return _expect_str(_MOLT_PATH_NORMPATH(path), "path_normpath")

    @staticmethod
    def expandvars(path: str) -> str:
        _require_cap("env.read")
        return _expect_str(_MOLT_PATH_EXPANDVARS(path), "path_expandvars")

    @staticmethod
    def abspath(path: str) -> str:
        return _expect_str(_MOLT_PATH_ABSPATH(path), "path_abspath")

    @staticmethod
    def relpath(path: str, start: str | None = None) -> str:
        return _expect_str(_MOLT_PATH_RELPATH(path, start), "path_relpath")

    @staticmethod
    def expanduser(path: str) -> str:
        _require_cap("env.read")
        return _expect_str(_MOLT_PATH_EXPANDUSER(path), "path_expanduser")

    @staticmethod
    def exists(path: Any) -> bool:
        _require_cap("fs.read")
        if callable(_MOLT_OS_PATH_EXISTS):
            return bool(_MOLT_OS_PATH_EXISTS(path))
        intrinsic = _require_callable_intrinsic(_MOLT_PATH_EXISTS, "molt_path_exists")
        return bool(intrinsic(path))

    @staticmethod
    def isdir(path: Any) -> bool:
        _require_cap("fs.read")
        if callable(_MOLT_OS_PATH_ISDIR):
            return bool(_MOLT_OS_PATH_ISDIR(path))
        intrinsic = _require_callable_intrinsic(_MOLT_PATH_ISDIR, "molt_path_isdir")
        return bool(intrinsic(path))

    @staticmethod
    def isfile(path: Any) -> bool:
        _require_cap("fs.read")
        if callable(_MOLT_OS_PATH_ISFILE):
            return bool(_MOLT_OS_PATH_ISFILE(path))
        intrinsic = _require_callable_intrinsic(_MOLT_PATH_ISFILE, "molt_path_isfile")
        return bool(intrinsic(path))

    @staticmethod
    def islink(path: Any) -> bool:
        _require_cap("fs.read")
        intrinsic = _require_callable_intrinsic(_MOLT_PATH_ISLINK, "molt_path_islink")
        return bool(intrinsic(path))

    @staticmethod
    def unlink(path: Any) -> None:
        _require_cap("fs.write")
        intrinsic = _require_callable_intrinsic(_MOLT_PATH_UNLINK, "molt_path_unlink")
        intrinsic(path)

    @staticmethod
    def rmdir(path: Any) -> None:
        _require_cap("fs.write")
        _MOLT_OS_RMDIR_V2(path)

    @staticmethod
    def commonpath(paths: Any) -> str:
        _require_cap("fs.read")
        intrinsic = _require_callable_intrinsic(
            _MOLT_OS_PATH_COMMONPATH, "molt_os_path_commonpath"
        )
        return _expect_str(intrinsic(list(paths)), "path_commonpath")

    @staticmethod
    def commonprefix(list_of_paths: Any) -> str:
        intrinsic = _require_callable_intrinsic(
            _MOLT_OS_PATH_COMMONPREFIX, "molt_os_path_commonprefix"
        )
        return _expect_str(intrinsic(list(list_of_paths)), "path_commonprefix")

    @staticmethod
    def getatime(path: Any) -> float:
        _require_cap("fs.read")
        intrinsic = _require_callable_intrinsic(
            _MOLT_OS_PATH_GETATIME, "molt_os_path_getatime"
        )
        return float(intrinsic(str(path)))

    @staticmethod
    def getctime(path: Any) -> float:
        _require_cap("fs.read")
        intrinsic = _require_callable_intrinsic(
            _MOLT_OS_PATH_GETCTIME, "molt_os_path_getctime"
        )
        return float(intrinsic(str(path)))

    @staticmethod
    def getmtime(path: Any) -> float:
        _require_cap("fs.read")
        intrinsic = _require_callable_intrinsic(
            _MOLT_OS_PATH_GETMTIME, "molt_os_path_getmtime"
        )
        return float(intrinsic(str(path)))

    @staticmethod
    def getsize(path: Any) -> int:
        _require_cap("fs.read")
        intrinsic = _require_callable_intrinsic(
            _MOLT_OS_PATH_GETSIZE, "molt_os_path_getsize"
        )
        return int(intrinsic(str(path)))

    @staticmethod
    def samefile(path1: Any, path2: Any) -> bool:
        _require_cap("fs.read")
        intrinsic = _require_callable_intrinsic(
            _MOLT_OS_PATH_SAMEFILE, "molt_os_path_samefile"
        )
        return bool(intrinsic(str(path1), str(path2)))

    @staticmethod
    def realpath(path: Any, *, strict: bool = False) -> str:
        return _expect_str(_MOLT_OS_PATH_REALPATH(str(path)), "path_realpath")


_path_impl = _Path()
path = _types.ModuleType("os.path")
for _name in dir(_path_impl):
    if _name.startswith("_"):
        continue
    setattr(path, _name, getattr(_path_impl, _name))
path.sep = sep
path.pathsep = pathsep
path.extsep = extsep
path.curdir = curdir
path.pardir = pardir
path.altsep = altsep
_sys.modules.setdefault("os.path", path)
_sys.modules.setdefault(f"{__name__}.path", path)
__path__ = []  # Enable dotted imports like `import os.path`.


def listdir(path: Any = ".") -> list[str]:
    _require_cap("fs.read")
    res = _MOLT_OS_LISTDIR_V2(path)
    if not isinstance(res, list):
        raise RuntimeError("os listdir intrinsic returned invalid value")
    if not all(isinstance(entry, str) for entry in res):
        raise RuntimeError("os listdir intrinsic returned invalid value")
    return list(res)


environ = _Environ()


def getpid() -> int:
    return int(_MOLT_OS_GETPID_V2())


def urandom(n: Any) -> bytes:
    _require_cap("rand")
    intrinsic = _require_callable_intrinsic(_MOLT_OS_URANDOM, "molt_os_urandom")
    return intrinsic(n)


def getcwd() -> str:
    _require_cap("fs.read")
    return str(_MOLT_OS_GETCWD_V2())


def getenv(key: str, default: Any = None) -> Any:
    _require_cap("env.read")
    return _molt_env_get(key, default)


def putenv(key: str, value: str) -> None:
    _require_cap("env.write")
    _molt_env_putenv(environ._check_key(key), environ._check_value(value))


def unsetenv(key: str) -> None:
    _require_cap("env.write")
    _molt_env_unsetenv(environ._check_key(key))


def unlink(path: Any) -> None:
    _Path.unlink(path)


def remove(path: Any) -> None:
    unlink(path)


def readlink(path: Any, *, dir_fd: int | None = None) -> str:
    _require_cap("fs.read")
    if dir_fd is not None:
        raise NotImplementedError("os.readlink(dir_fd=...) is not supported")
    return _expect_str(_MOLT_OS_READLINK_V2(path), "readlink")


def symlink(
    src: Any,
    dst: Any,
    target_is_directory: bool = False,
    *,
    dir_fd: int | None = None,
) -> None:
    _require_cap("fs.write")
    if dir_fd is not None:
        raise NotImplementedError("os.symlink(dir_fd=...) is not supported")
    _MOLT_OS_SYMLINK_V2(src, dst)


def rmdir(path: Any) -> None:
    _Path.rmdir(path)


def mkdir(path: Any, mode: int = 0o777) -> None:
    _require_cap("fs.write")
    _MOLT_OS_MKDIR_V2(path, mode)


def chmod(path: Any, mode: int) -> None:
    _require_cap("fs.write")
    _MOLT_OS_CHMOD_V2(path, mode)


def makedirs(name: Any, mode: int = 0o777, exist_ok: bool = False) -> None:
    path = fspath(name)
    if callable(_MOLT_OS_MAKEDIRS):
        _MOLT_OS_MAKEDIRS(path, mode, bool(exist_ok))
        return
    intrinsic = _require_callable_intrinsic(_MOLT_PATH_MAKEDIRS, "molt_path_makedirs")
    intrinsic(path, mode, bool(exist_ok))


def open(path: Any, flags: int, mode: int = 0o777) -> int:
    _require_cap("fs.write")
    result = _MOLT_OS_OPEN(path, flags, mode)
    if isinstance(result, int):
        return result
    raise RuntimeError("os open intrinsic returned invalid value")


def fdopen(fd: int, mode: str = "r", closefd: bool = True) -> Any:
    intrinsic = _require_callable_intrinsic(_MOLT_OS_FDOPEN, "molt_os_fdopen")
    return intrinsic(int(fd), str(mode), bool(closefd))


def close(fd: int) -> None:
    intrinsic = _require_callable_intrinsic(_MOLT_OS_CLOSE, "molt_os_close")
    intrinsic(fd)


def read(fd: int, n: int) -> bytes:
    intrinsic = _require_callable_intrinsic(_MOLT_OS_READ, "molt_os_read")
    result = intrinsic(fd, n)
    if isinstance(result, (bytes, bytearray, memoryview)):
        return bytes(result)
    raise RuntimeError("os read intrinsic returned invalid value")


def write(fd: int, data: Any) -> int:
    intrinsic = _require_callable_intrinsic(_MOLT_OS_WRITE, "molt_os_write")
    return int(intrinsic(fd, data))


def pipe() -> tuple[int, int]:
    intrinsic = _require_callable_intrinsic(_MOLT_OS_PIPE, "molt_os_pipe")
    pair = intrinsic()
    if not isinstance(pair, (tuple, list)) or len(pair) != 2:
        raise RuntimeError("os pipe intrinsic returned invalid value")
    read_fd, write_fd = pair[0], pair[1]
    return int(read_fd), int(write_fd)


def dup(fd: int) -> int:
    intrinsic = _require_callable_intrinsic(_MOLT_OS_DUP, "molt_os_dup")
    return int(intrinsic(fd))


def dup2(fd: int, fd2: int) -> int:
    return int(_MOLT_OS_DUP2(int(fd), int(fd2)))


def lseek(fd: int, pos: int, how: int) -> int:
    return int(_MOLT_OS_LSEEK(int(fd), int(pos), int(how)))


def ftruncate(fd: int, length: int) -> None:
    _MOLT_OS_FTRUNCATE(int(fd), int(length))


def isatty(fd: int) -> bool:
    return bool(_MOLT_OS_ISATTY(int(fd)))


def get_inheritable(fd: int) -> bool:
    intrinsic = _require_callable_intrinsic(
        _MOLT_OS_GET_INHERITABLE, "molt_os_get_inheritable"
    )
    return bool(intrinsic(fd))


def set_inheritable(fd: int, inheritable: bool) -> None:
    intrinsic = _require_callable_intrinsic(
        _MOLT_OS_SET_INHERITABLE, "molt_os_set_inheritable"
    )
    intrinsic(fd, bool(inheritable))


def stat(
    path: Any, *, dir_fd: int | None = None, follow_symlinks: bool = True
) -> stat_result:
    _require_cap("fs.read")
    if dir_fd is not None:
        raise NotImplementedError("os.stat(dir_fd=...) is not supported")
    if bool(follow_symlinks):
        intrinsic = _require_os_intrinsic("molt_os_stat")
        return _expect_stat_result(intrinsic(path), "stat")
    intrinsic = _require_os_intrinsic("molt_os_lstat")
    return _expect_stat_result(intrinsic(path), "lstat")


def lstat(path: Any, *, dir_fd: int | None = None) -> stat_result:
    _require_cap("fs.read")
    if dir_fd is not None:
        raise NotImplementedError("os.lstat(dir_fd=...) is not supported")
    intrinsic = _require_os_intrinsic("molt_os_lstat")
    return _expect_stat_result(intrinsic(path), "lstat")


def fstat(fd: int) -> stat_result:
    _require_cap("fs.read")
    intrinsic = _require_os_intrinsic("molt_os_fstat")
    return _expect_stat_result(intrinsic(fd), "fstat")


def rename(
    src: Any,
    dst: Any,
    *,
    src_dir_fd: int | None = None,
    dst_dir_fd: int | None = None,
) -> None:
    _require_cap("fs.write")
    if src_dir_fd is not None:
        raise NotImplementedError("os.rename(src_dir_fd=...) is not supported")
    if dst_dir_fd is not None:
        raise NotImplementedError("os.rename(dst_dir_fd=...) is not supported")
    intrinsic = _require_os_intrinsic("molt_os_rename")
    intrinsic(src, dst)


def replace(
    src: Any,
    dst: Any,
    *,
    src_dir_fd: int | None = None,
    dst_dir_fd: int | None = None,
) -> None:
    _require_cap("fs.write")
    if src_dir_fd is not None:
        raise NotImplementedError("os.replace(src_dir_fd=...) is not supported")
    if dst_dir_fd is not None:
        raise NotImplementedError("os.replace(dst_dir_fd=...) is not supported")
    intrinsic = _require_os_intrinsic("molt_os_replace")
    intrinsic(src, dst)


def access(path: Any, mode: int, *, follow_symlinks: bool = True) -> bool:
    _require_cap("fs.read")
    intrinsic = _require_callable_intrinsic(_MOLT_OS_ACCESS, "molt_os_access")
    return bool(intrinsic(str(path), mode))


def chdir(path: Any) -> None:
    _require_cap("fs.write")
    intrinsic = _require_callable_intrinsic(_MOLT_OS_CHDIR, "molt_os_chdir")
    intrinsic(str(path))


def cpu_count() -> int | None:
    intrinsic = _require_callable_intrinsic(_MOLT_OS_CPU_COUNT, "molt_os_cpu_count")
    result = intrinsic()
    if result is None:
        return None
    return int(result)


def link(
    src: Any,
    dst: Any,
    *,
    src_dir_fd: int | None = None,
    dst_dir_fd: int | None = None,
    follow_symlinks: bool = True,
) -> None:
    _require_cap("fs.write")
    if src_dir_fd is not None:
        raise NotImplementedError("os.link(src_dir_fd=...) is not supported")
    if dst_dir_fd is not None:
        raise NotImplementedError("os.link(dst_dir_fd=...) is not supported")
    intrinsic = _require_callable_intrinsic(_MOLT_OS_LINK, "molt_os_link")
    intrinsic(str(src), str(dst))


def truncate(path: Any, length: int) -> None:
    _require_cap("fs.write")
    intrinsic = _require_callable_intrinsic(_MOLT_OS_TRUNCATE, "molt_os_truncate")
    intrinsic(str(path), length)


def umask(mask: int) -> int:
    intrinsic = _require_callable_intrinsic(_MOLT_OS_UMASK, "molt_os_umask")
    return int(intrinsic(mask))


def uname() -> uname_result:
    intrinsic = _require_callable_intrinsic(_MOLT_OS_UNAME, "molt_os_uname")
    result = intrinsic()
    if not isinstance(result, (tuple, list)) or len(result) != 5:
        raise RuntimeError("os uname intrinsic returned invalid value")
    return uname_result(
        _expect_str(result[0], "uname"),
        _expect_str(result[1], "uname"),
        _expect_str(result[2], "uname"),
        _expect_str(result[3], "uname"),
        _expect_str(result[4], "uname"),
    )


def getppid() -> int:
    intrinsic = _require_callable_intrinsic(_MOLT_OS_GETPPID, "molt_os_getppid")
    return int(intrinsic())


def getuid() -> int:
    intrinsic = _require_callable_intrinsic(_MOLT_OS_GETUID, "molt_os_getuid")
    return int(intrinsic())


def getgid() -> int:
    intrinsic = _require_callable_intrinsic(_MOLT_OS_GETGID, "molt_os_getgid")
    return int(intrinsic())


def geteuid() -> int:
    intrinsic = _require_callable_intrinsic(_MOLT_OS_GETEUID, "molt_os_geteuid")
    return int(intrinsic())


def getegid() -> int:
    intrinsic = _require_callable_intrinsic(_MOLT_OS_GETEGID, "molt_os_getegid")
    return int(intrinsic())


def getlogin() -> str:
    _require_cap("env.read")
    intrinsic = _require_callable_intrinsic(_MOLT_OS_GETLOGIN, "molt_os_getlogin")
    return _expect_str(intrinsic(), "getlogin")


def getloadavg() -> tuple[float, float, float]:
    intrinsic = _require_callable_intrinsic(_MOLT_OS_GETLOADAVG, "molt_os_getloadavg")
    result = intrinsic()
    if not isinstance(result, (tuple, list)) or len(result) != 3:
        raise RuntimeError("os getloadavg intrinsic returned invalid value")
    return (float(result[0]), float(result[1]), float(result[2]))


def kill(pid: int, sig: int) -> None:
    _MOLT_OS_KILL(int(pid), int(sig))


def waitpid(pid: int, options: int) -> tuple[int, int]:
    result = _MOLT_OS_WAITPID(int(pid), int(options))
    return (int(result[0]), int(result[1]))


def getpgrp() -> int:
    return int(_MOLT_OS_GETPGRP())


def setpgrp() -> None:
    _MOLT_OS_SETPGRP()


def setsid() -> int:
    return int(_MOLT_OS_SETSID())


def sysconf(name: int | str) -> int:
    if isinstance(name, str):
        _name = sysconf_names.get(name, -1)
        if _name == -1:
            raise ValueError("unrecognized configuration name")
        name = _name
    return int(_MOLT_OS_SYSCONF(int(name)))


def utime(
    path: Any,
    times: tuple[float, float] | None = None,
    *,
    ns: tuple[int, int] | None = None,
    dir_fd: int | None = None,
    follow_symlinks: bool = True,
) -> None:
    if dir_fd is not None:
        raise NotImplementedError("os.utime(dir_fd=...) is not supported")
    if ns is not None:
        raise NotImplementedError("os.utime(ns=...) is not supported")
    if times is not None:
        _MOLT_OS_UTIME(str(path), float(times[0]), float(times[1]))
    else:
        _MOLT_OS_UTIME(str(path), None, None)


def sendfile(out_fd: int, in_fd: int, offset: int | None, count: int) -> int:
    return int(_MOLT_OS_SENDFILE(int(out_fd), int(in_fd), offset, int(count)))


def removedirs(name: Any) -> None:
    _require_cap("fs.write")
    intrinsic = _require_callable_intrinsic(_MOLT_OS_REMOVEDIRS, "molt_os_removedirs")
    intrinsic(str(name))


def get_terminal_size(fd: int = 1) -> terminal_size:
    intrinsic = _require_callable_intrinsic(
        _MOLT_OS_GET_TERMINAL_SIZE, "molt_os_get_terminal_size"
    )
    result = intrinsic(fd)
    if not isinstance(result, (tuple, list)) or len(result) != 2:
        raise RuntimeError("os get_terminal_size intrinsic returned invalid value")
    return terminal_size((int(result[0]), int(result[1])))


def walk(
    top: Any,
    topdown: bool = True,
    onerror: Any = None,
    followlinks: bool = False,
) -> Any:
    _require_cap("fs.read")
    intrinsic = _require_callable_intrinsic(_MOLT_OS_WALK, "molt_os_walk")
    result = intrinsic(str(top), bool(topdown), bool(followlinks))
    if not isinstance(result, (list, tuple)):
        raise RuntimeError("os walk intrinsic returned invalid value")
    for entry in result:
        yield entry


def scandir(path: Any = ".") -> list:
    _require_cap("fs.read")
    intrinsic = _require_callable_intrinsic(_MOLT_OS_SCANDIR, "molt_os_scandir")
    raw = intrinsic(str(path))
    if not isinstance(raw, (list, tuple)):
        raise RuntimeError("os scandir intrinsic returned invalid value")
    entries: list[DirEntry] = []
    for item in raw:
        if not isinstance(item, (tuple, list)) or len(item) < 5:
            raise RuntimeError("os scandir intrinsic returned invalid entry")
        entries.append(
            DirEntry(
                name=str(item[0]),
                path=str(item[1]),
                is_dir=bool(item[2]),
                is_file=bool(item[3]),
                is_symlink=bool(item[4]),
            )
        )
    return entries


# ---------------------------------------------------------------------------
# Namespace cleanup — remove names that are not part of CPython's os public API.
# These are needed for type annotations and internal helpers but must not appear
# in the module __dict__ as non-underscore public names.
# ---------------------------------------------------------------------------
for _name in (
    "TYPE_CHECKING",
    "Any",
    "Iterator",
    "ItemsView",
    "KeysView",
    "ValuesView",
):
    globals().pop(_name, None)
