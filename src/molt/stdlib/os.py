"""Minimal os shim for Molt."""

from __future__ import annotations

import abc as _abc
import sys as _sys
from sys import _struct_sequence_type, _struct_sequence_new
import types as _types
from _collections_abc import MutableMapping as _MutableMapping, Mapping as _Mapping

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


_MOLT_SYS_VERSION_INFO = _require_intrinsic("molt_sys_version_info")
_RUNTIME_VERSION = _MOLT_SYS_VERSION_INFO()
_PYTHON_313 = _RUNTIME_VERSION >= (3, 13)

_MOLT_ENV_MAPPING_INIT = _require_intrinsic("molt_env_mapping_init")
_MOLT_ENV_MAPPING_GET = _require_intrinsic("molt_env_mapping_get")
_MOLT_ENV_MAPPING_SET = _require_intrinsic("molt_env_mapping_set")
_MOLT_ENV_MAPPING_DELETE = _require_intrinsic("molt_env_mapping_delete")
_MOLT_ENV_MAPPING_KEYS = _require_intrinsic("molt_env_mapping_keys")
_MOLT_ENV_MAPPING_LEN = _require_intrinsic("molt_env_mapping_len")
_MOLT_ENV_MAPPING_ITEMS_ITER = _require_intrinsic("molt_env_mapping_items_iter")
_MOLT_ENV_PUTENV = _require_intrinsic("molt_env_putenv")
_MOLT_ENV_UNSETENV = _require_intrinsic("molt_env_unsetenv")
_MOLT_OS_NAME = _require_intrinsic("molt_os_name")
_MOLT_OS_STAT_RESULT_TYPE = _require_intrinsic("molt_os_stat_result_type")
_MOLT_PATH_EXISTS = _require_intrinsic("molt_path_exists")
_MOLT_PATH_UNLINK = _require_intrinsic("molt_path_unlink")
_MOLT_PATH_JOIN_MANY = _require_intrinsic("molt_path_join_many")
_MOLT_PATH_ISABS = _require_intrinsic("molt_path_isabs")
_MOLT_PATH_DIRNAME = _require_intrinsic("molt_path_dirname")
_MOLT_PATH_BASENAME = _require_intrinsic("molt_path_basename")
_MOLT_PATH_SPLIT = _require_intrinsic("molt_path_split")
_MOLT_PATH_SPLITEXT = _require_intrinsic("molt_path_splitext")
_MOLT_PATH_NORMPATH = _require_intrinsic("molt_path_normpath")
_MOLT_PATH_ABSPATH = _require_intrinsic("molt_path_abspath")
_MOLT_PATH_RELPATH = _require_intrinsic("molt_path_relpath")
_MOLT_PATH_ISDIR = _require_intrinsic("molt_path_isdir")
_MOLT_PATH_ISFILE = _require_intrinsic("molt_path_isfile")
_MOLT_PATH_ISLINK = _require_intrinsic("molt_path_islink")
_MOLT_OS_STAT = _require_intrinsic("molt_os_stat")
_MOLT_OS_LSTAT = _require_intrinsic("molt_os_lstat")
_MOLT_OS_FSTAT = _require_intrinsic("molt_os_fstat")
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
_MOLT_OS_ACCESS = _require_intrinsic("molt_os_access")
_MOLT_OS_CHDIR = _require_intrinsic("molt_os_chdir")
_MOLT_OS_CPU_COUNT = _require_intrinsic("molt_os_cpu_count")
_MOLT_OS_LINK = _require_intrinsic("molt_os_link")
_MOLT_OS_MAKEDIRS = _require_intrinsic("molt_os_makedirs")
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
_MOLT_PATH_SPLITROOT = _require_intrinsic("molt_path_splitroot")
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
_MOLT_PATH_READLINK = _require_intrinsic("molt_path_readlink")
_MOLT_CAP_REQUIRE = _require_intrinsic("molt_capabilities_require")
_MOLT_REQUIRE_OPERATION = _require_intrinsic("molt_capabilities_require_operation")
_MOLT_OS_WIFEXITED = _require_intrinsic("molt_os_wifexited")
_MOLT_OS_WEXITSTATUS = _require_intrinsic("molt_os_wexitstatus")
_MOLT_OS_WIFSIGNALED = _require_intrinsic("molt_os_wifsignaled")
_MOLT_OS_WTERMSIG = _require_intrinsic("molt_os_wtermsig")
_MOLT_OS_WIFSTOPPED = _require_intrinsic("molt_os_wifstopped")
_MOLT_OS_WSTOPSIG = _require_intrinsic("molt_os_wstopsig")
_MOLT_OS_FSPATH = _require_intrinsic("molt_os_fspath")
_MOLT_OS_STAT_AT = _require_intrinsic("molt_os_stat_at")
_MOLT_OS_LSTAT_AT = _require_intrinsic("molt_os_lstat_at")
_MOLT_OS_READLINK_AT = _require_intrinsic("molt_os_readlink_at")
_MOLT_OS_SYMLINK_AT = _require_intrinsic("molt_os_symlink_at")
_MOLT_OS_RENAME_AT = _require_intrinsic("molt_os_rename_at")
_MOLT_OS_REPLACE_AT = _require_intrinsic("molt_os_replace_at")
_MOLT_OS_LINK_AT = _require_intrinsic("molt_os_link_at")
_MOLT_OS_UTIME_AT = _require_intrinsic("molt_os_utime_at")
_MOLT_SUBPROCESS_RUN = _require_intrinsic("molt_subprocess_run")


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
    "system",
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
    "fsdecode",
    "fdopen",
    "supports_dir_fd",
    "supports_follow_symlinks",
]

name = _resolve_os_name()
_WINDOWS_UTF16_ROOT = name == "nt" and _PYTHON_313
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

# Runtime target capability owns both namespace admission and numeric names.
_sysconf_names_raw = _MOLT_OS_SYSCONF_NAMES()
if _sysconf_names_raw is not None:
    if not isinstance(_sysconf_names_raw, (list, tuple)) or len(_sysconf_names_raw) % 2:
        raise RuntimeError("os sysconf_names intrinsic returned invalid table")
    sysconf_names: dict[str, int] = {}
    _i = 0
    while _i < len(_sysconf_names_raw):
        _k = _sysconf_names_raw[_i]
        _v = _sysconf_names_raw[_i + 1]
        if not isinstance(_k, str) or not isinstance(_v, int):
            raise RuntimeError("os sysconf_names intrinsic returned invalid entry")
        sysconf_names[_k] = _v
        _i += 2
    del _i
else:
    __all__.remove("sysconf")
    __all__.remove("sysconf_names")

_ENV_MISSING = object()

if TYPE_CHECKING:

    def _molt_getcwd() -> str:
        return curdir

    def _molt_path_exists(path: Any) -> bool:
        return False

    def _molt_path_unlink(path: Any) -> None:
        return None

    def _molt_path_rmdir(path: Any) -> None:
        return None


def _require_cap(name: str) -> None:
    _MOLT_CAP_REQUIRE(name)
    return None


def _require_callable_intrinsic(value: Any, name: str):
    if not callable(value):
        raise RuntimeError(f"intrinsic unavailable: {name}")
    return value


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


# The native producer owns the compile-target stat fields and their exact order.
# Captured types are provider state; rebinding public module attributes cannot
# change the result type of already initialized native producers.
stat_result = _MOLT_OS_STAT_RESULT_TYPE()
uname_result = _struct_sequence_type(
    "uname_result",
    ("sysname", "nodename", "release", "version", "machine"),
    module=name,
    static_origin=False,
    docs=(
        "operating system name",
        "name of machine on network (implementation-defined)",
        "operating system release",
        "operating system version",
        "hardware identifier",
    ),
    doc="uname_result: Result from os.uname().\n\nThis object may be accessed either as a tuple of\n  (sysname, nodename, release, version, machine),\nor via the attributes sysname, nodename, release, version, and machine.\n\nSee os.uname for more information.",
)
_UNAME_RESULT = uname_result
terminal_size = _struct_sequence_type(
    "terminal_size",
    ("columns", "lines"),
    module="os",
    static_origin=False,
    docs=(
        "width of the terminal window in characters",
        "height of the terminal window in characters",
    ),
    doc="A tuple of (columns, lines) for holding terminal window size",
)
_TERMINAL_SIZE = terminal_size


class DirEntry:
    """Entry yielded by os.scandir()."""

    __slots__ = (
        "name",
        "path",
        "_is_dir",
        "_is_file",
        "_is_symlink",
        "_stat",
        "_lstat",
        "_inode",
    )

    def __init__(
        self,
        name: str | bytes,
        path: str | bytes,
        is_dir: bool,
        is_file: bool,
        is_symlink: bool,
    ) -> None:
        self.name = name
        self.path = path
        self._is_dir = is_dir
        self._is_file = is_file
        self._is_symlink = is_symlink
        # CPython caches each stat fact on the entry (Modules/posixmodule.c:
        # DirEntry_get_lstat, os_DirEntry_stat_impl, os_DirEntry_inode_impl).
        self._stat: "stat_result | None" = None
        self._lstat: "stat_result | None" = None
        self._inode: int | None = None

    def is_dir(self, *, follow_symlinks: bool = True) -> bool:
        return self._is_dir

    def is_file(self, *, follow_symlinks: bool = True) -> bool:
        return self._is_file

    def is_symlink(self) -> bool:
        return self._is_symlink

    def stat(self, *, follow_symlinks: bool = True) -> "stat_result":
        if not follow_symlinks:
            return self._cached_lstat()
        if self._stat is None:
            if self._is_symlink:
                _require_cap("fs.read")
                intrinsic = _require_callable_intrinsic(
                    _require_intrinsic("molt_os_stat"), "molt_os_stat"
                )
                self._stat = intrinsic(self.path)
            else:
                # A non-symlink's stat is the same object as its lstat.
                self._stat = self._cached_lstat()
        return self._stat

    def inode(self) -> int:
        if self._inode is None:
            self._inode = self._query_lstat().st_ino
        return self._inode

    def _cached_lstat(self) -> "stat_result":
        if self._lstat is None:
            result = self._query_lstat()
            if name == "nt":
                # CPython answers lstat from the directory listing, which
                # carries no file identity (find_data_to_file_info): st_ino,
                # st_dev and st_nlink are zero. inode() still reads the file.
                cls, (fields, extra) = result.__reduce__()
                result = cls((fields[0], 0, 0, 0, *fields[4:]), extra)
            self._lstat = result
        return self._lstat

    def _query_lstat(self) -> "stat_result":
        _require_cap("fs.read")
        intrinsic = _require_callable_intrinsic(
            _require_intrinsic("molt_os_lstat"), "molt_os_lstat"
        )
        return intrinsic(self.path)

    def __repr__(self) -> str:
        return f"<DirEntry {self.name!r}>"

    def __fspath__(self) -> str | bytes:
        return self.path


class _Environ(_MutableMapping):
    # Codecs remain ordinary Python attributes; storage is exclusively runtime
    # owned so neither this facade nor a view can expose an unchecked dict.
    def __init__(self, encodekey, decodekey, encodevalue, decodevalue):
        self.encodekey = encodekey
        self.decodekey = decodekey
        self.encodevalue = encodevalue
        self.decodevalue = decodevalue

    def __getitem__(self, key):
        _MOLT_REQUIRE_OPERATION("env.get")
        try:
            value = _MOLT_ENV_MAPPING_GET(self.encodekey(key))
        except KeyError:
            raise KeyError(key) from None
        return self.decodevalue(value)

    def __setitem__(self, key, value):
        _MOLT_REQUIRE_OPERATION("env.set")
        key = self.encodekey(key)
        value = self.encodevalue(value)
        _MOLT_ENV_MAPPING_SET(key, value)

    def __delitem__(self, key):
        _MOLT_REQUIRE_OPERATION("env.unset")
        encodedkey = self.encodekey(key)
        try:
            _MOLT_ENV_MAPPING_DELETE(encodedkey)
        except KeyError:
            raise KeyError(key) from None

    def __iter__(self):
        # A generator captures only encoded key references at first advance.
        # Decoding stays per-yield; view construction performs no snapshot.
        _MOLT_REQUIRE_OPERATION("env.keys")
        keys = _MOLT_ENV_MAPPING_KEYS()
        for key in keys:
            yield self.decodekey(key)

    def __len__(self):
        _MOLT_REQUIRE_OPERATION("env.len")
        return _MOLT_ENV_MAPPING_LEN()

    def __repr__(self):
        _MOLT_REQUIRE_OPERATION("env.snapshot")
        formatted_items = ", ".join(
            f"{self.decodekey(key)!r}: {self.decodevalue(value)!r}"
            for key, value in _MOLT_ENV_MAPPING_ITEMS_ITER()
        )
        return f"environ({{{formatted_items}}})"

    def copy(self):
        _MOLT_REQUIRE_OPERATION("env.snapshot")
        return dict(self)

    def setdefault(self, key, value):
        _MOLT_REQUIRE_OPERATION("env.setdefault")
        if key not in self:
            self[key] = value
        return self[key]

    def pop(self, key, default=_ENV_MISSING):
        _MOLT_REQUIRE_OPERATION("env.pop")
        if default is _ENV_MISSING:
            return _MutableMapping.pop(self, key)
        return _MutableMapping.pop(self, key, default)

    def popitem(self):
        _MOLT_REQUIRE_OPERATION("env.popitem")
        return _MutableMapping.popitem(self)

    def clear(self):
        _MOLT_REQUIRE_OPERATION("env.clear")
        return _MutableMapping.clear(self)

    def update(self, other=(), /, **kwargs):
        _MOLT_REQUIRE_OPERATION("env.set")
        return _MutableMapping.update(self, other, **kwargs)

    def keys(self):
        _MOLT_REQUIRE_OPERATION("env.keys")
        return _MutableMapping.keys(self)

    def items(self):
        _MOLT_REQUIRE_OPERATION("env.snapshot")
        return _MutableMapping.items(self)

    def values(self):
        _MOLT_REQUIRE_OPERATION("env.snapshot")
        return _MutableMapping.values(self)

    def __ior__(self, other):
        self.update(other)
        return self

    def __or__(self, other):
        if not isinstance(other, _Mapping):
            return NotImplemented
        new = dict(self)
        new.update(other)
        return new

    def __ror__(self, other):
        if not isinstance(other, _Mapping):
            return NotImplemented
        new = dict(other)
        new.update(self)
        return new


def _createenviron():
    if name == "nt":

        def encode(value):
            if not isinstance(value, str):
                raise TypeError("str expected, not %s" % type(value).__name__)
            return value

        decode = str

        def encodekey(key):
            return encode(key).upper()
    else:
        encoding = _sys.getfilesystemencoding()

        def encode(value):
            if not isinstance(value, str):
                raise TypeError("str expected, not %s" % type(value).__name__)
            return value.encode(encoding, "surrogateescape")

        def decode(value):
            return value.decode(encoding, "surrogateescape")

        encodekey = encode
    return _Environ(encodekey, decode, encode, decode)


class PathLike(_abc.ABC):
    __slots__ = ()

    @_abc.abstractmethod
    def __fspath__(self) -> str | bytes: ...


def fspath(path: Any) -> str | bytes:
    return _MOLT_OS_FSPATH(path)


def _fscodec():
    # CPython captures the target filesystem policy when os is initialized.
    # Later sys monkeypatches or path callbacks must not change that policy.
    encoding = _sys.getfilesystemencoding()
    errors = _sys.getfilesystemencodeerrors()

    def fsencode(filename):
        filename = fspath(filename)
        if isinstance(filename, str):
            return filename.encode(encoding, errors)
        return filename

    def fsdecode(filename):
        filename = fspath(filename)
        if isinstance(filename, bytes):
            return filename.decode(encoding, errors)
        return filename

    return fsencode, fsdecode


fsencode, fsdecode = _fscodec()
del _fscodec


# CPython supported-patch path protocols (3.12.15 through 3.15.0).
# Regex import, compilation and each str/bytes cache stay at first expansion.


def _posix_expanduser(path):
    """Expand ~ and ~user constructions.  If user or $HOME is unknown,
    do nothing."""
    path = fspath(path)
    if isinstance(path, bytes):
        tilde = b"~"
    else:
        tilde = "~"
    if not path.startswith(tilde):
        return path
    sep = b"/" if isinstance(path, bytes) else "/"
    i = path.find(sep, 1)
    if i < 0:
        i = len(path)
    if i == 1:
        _MOLT_REQUIRE_OPERATION("env.expanduser")
        if "HOME" not in environ:
            try:
                import pwd
            except ImportError:
                return path
            try:
                userhome = pwd.getpwuid(getuid()).pw_dir
            except KeyError:
                return path
        else:
            userhome = environ["HOME"]
    else:
        try:
            import pwd
        except ImportError:
            return path
        name = path[1:i]
        if isinstance(name, bytes):
            name = fsdecode(name)
        try:
            pwent = pwd.getpwnam(name)
        except KeyError:
            return path
        userhome = pwent.pw_dir
    if userhome is None and _sys.platform == "vxworks":
        return path
    if isinstance(path, bytes):
        userhome = fsencode(userhome)
        root = b"/"
    else:
        root = "/"
    userhome = userhome.rstrip(root)
    return userhome + path[i:] or root


def _expandvars(path, windows):
    path = fspath(path)
    raw = isinstance(path, bytes)
    if raw:
        if b"$" not in path and (not windows or b"%" not in path):
            return path
    elif "$" not in path and (not windows or "%" not in path):
        return path
    _MOLT_REQUIRE_OPERATION("env.expandvars")
    environment = globals().get("environb") if raw else environ

    # CPython's ASCII expansion grammar has only ASCII delimiters. Scan byte
    # coordinates so UTF-8 character indexing cannot repeatedly walk a prefix.
    # Base operations bypass subclass hooks, just as the regex engine does.
    if raw:
        data = bytes.__bytes__(path)
    else:
        data = str.encode(path, "utf-8", "surrogatepass")
    empty = b"" if raw else ""

    def text(segment):
        return segment if raw else segment.decode("utf-8", "surrogatepass")

    parts = []
    literal_start = 0
    index = 0
    size = len(data)
    while index < size:
        if not windows:
            index = data.find(b"$", index)
            if index < 0:
                break
        char = data[index]
        if windows and char == 39:  # Single quotes protect the entire span.
            start = index
            end = data.find(b"'", index + 1)
            index = size if end < 0 else end + 1
            if literal_start < start:
                parts.append(text(data[literal_start:start]))
            parts.append(text(data[start:index]))
            literal_start = index
            continue
        if char != 36 and (not windows or char != 37):
            index += 1
            continue
        start = index
        index += 1
        if windows and index < size and data[index] == char:
            # NT alone treats $$ and %% as escaped literal markers.
            index += 1
            value = text(data[start : start + 1])
        else:
            if char == 37:
                end = data.find(b"%", index)
                if end < 0:
                    break
                key = data[index:end]
                index = end + 1
            elif index < size and data[index] == 123:
                end = data.find(b"}", index + 1)
                if end < 0:
                    break
                key = data[index + 1 : end]
                index = end + 1
            else:
                name_start = index
                while index < size:
                    char = data[index]
                    if not (
                        65 <= char <= 90
                        or 97 <= char <= 122
                        or 48 <= char <= 57
                        or char == 95
                        or (windows and char == 45)
                    ):
                        break
                    index += 1
                if index == name_start:
                    continue
                key = data[name_start:index]
            key = text(key)
            try:
                if environment is None:
                    value = fsencode(environ[fsdecode(key)])
                else:
                    value = environment[key]
            except KeyError:
                value = text(data[start:index])
        if literal_start < start:
            parts.append(text(data[literal_start:start]))
        # Match CPython substitution: None disappears, and other replacement
        # types are checked by the final join, after all mapping callbacks.
        if value is not None:
            parts.append(value)
        literal_start = index
    if literal_start < size:
        parts.append(text(data[literal_start:]))
    return empty.join(parts)


def _posix_expandvars(path):
    return _expandvars(path, False)


def _path_nt_split(p):
    """Split a pathname.

    Return tuple (head, tail) where tail is everything after the final slash.
    Either part may be empty."""
    p = fspath(p)
    seps = b"\\/" if isinstance(p, bytes) else "\\/"
    d, r, p = _path_splitroot(p, False)
    i = len(p)
    while i and p[i - 1] not in seps:
        i -= 1
    head, tail = (p[:i], p[i:])
    return (d + r + head.rstrip(seps), tail)


def _nt_expanduser(path):
    """Expand ~ and ~user constructs.

    If user or $HOME is unknown, do nothing."""
    path = fspath(path)
    if isinstance(path, bytes):
        tilde = b"~"
    else:
        tilde = "~"
    if not path.startswith(tilde):
        return path
    i, n = (1, len(path))
    while i < n and path[i] not in (b"\\/" if isinstance(path, bytes) else "\\/"):
        i += 1
    _MOLT_REQUIRE_OPERATION("env.expanduser")
    if "USERPROFILE" in environ:
        userhome = environ["USERPROFILE"]
    elif not "HOMEPATH" in environ:
        return path
    else:
        try:
            drive = environ["HOMEDRIVE"]
        except KeyError:
            drive = ""
        userhome = _Path.join(drive, environ["HOMEPATH"])
    if i != 1:
        target_user = path[1:i]
        if isinstance(target_user, bytes):
            target_user = fsdecode(target_user)
        current_user = environ.get("USERNAME")
        if target_user != current_user:
            if current_user != _Path.basename(userhome):
                return path
            userhome = _Path.join(_Path.dirname(userhome), target_user)
    if isinstance(path, bytes):
        userhome = fsencode(userhome)
    return userhome + path[i:]


def _nt_expandvars(path):
    return _expandvars(path, True)


_path_expandvars = _nt_expandvars if name == "nt" else _posix_expandvars
_path_expanduser = _nt_expanduser if name == "nt" else _posix_expanduser


def _path_splitroot(value, posix):
    # One legacy callback authority for both lexical flavors. Exact values and
    # modern native providers use the shared native parser; legacy subclasses
    # retain the actual CPython slicing/replace/find result semantics.
    value = fspath(value)
    wide = _WINDOWS_UTF16_ROOT and not posix
    if wide or (posix and _PYTHON_313) or type(value) in (str, bytes):
        return _MOLT_PATH_SPLITROOT(value, posix, wide, False)
    if posix:
        slash, empty = (b"/", b"") if isinstance(value, bytes) else ("/", "")
        if value[:1] != slash:
            return empty, empty, value
        if value[1:2] != slash or value[2:3] == slash:
            return empty, slash, value[1:]
        return empty, value[:2], value[2:]
    if isinstance(value, bytes):
        slash, alt, colon, unc, empty = b"\\", b"/", b":", b"\\\\?\\UNC\\", b""
    else:
        slash, alt, colon, unc, empty = "\\", "/", ":", "\\\\?\\UNC\\", ""
    normalized = value.replace(alt, slash)
    if normalized[:1] == slash:
        if normalized[1:2] == slash:
            start = 8 if normalized[:8].upper() == unc else 2
            end = normalized.find(slash, start)
            if end == -1:
                return value, empty, empty
            end = normalized.find(slash, end + 1)
            if end == -1:
                return value, empty, empty
            return value[:end], value[end : end + 1], value[end + 1 :]
        return empty, value[:1], value[1:]
    if normalized[1:2] == colon:
        if normalized[2:3] == slash:
            return value[:2], value[2:3], value[3:]
        return value[:2], empty, value[2:]
    return empty, empty, value


def _path_join(first, rest, posix):
    sep = "/" if posix else "\\"
    # CPython 3.12 posixpath/ntpath ordering. Native accumulation handles
    # exact values, including lazily resolved PathLike operands, in linear
    # time. A subclass resumes here once, before any observable operation.
    first = fspath(first)
    slash = (
        (b"/" if isinstance(first, bytes) else "/")
        if sep == "/"
        else (b"\\" if isinstance(first, bytes) else "\\")
    )
    try:
        result = _MOLT_PATH_JOIN_MANY(first, rest, posix)
        if not isinstance(result, tuple):
            return result
        resume, b, state = result
        del result  # Do not keep a second owner of callback-bearing values.
        if sep == "/":
            path = first if state is None else state
            del state
            if not rest:
                path[:0] + slash
            for index in range(resume, len(rest)):
                if index != resume or b is None:
                    b = fspath(rest[index])
                if b.startswith(slash):
                    path = b
                elif not path or path.endswith(slash):
                    path += b
                else:
                    path += slash + b
            return path
        seps = b"\\/" if isinstance(first, bytes) else "\\/"
        colon = b":" if isinstance(first, bytes) else ":"
        if state is None:
            if not rest:
                first[:0] + slash
            drive, root, tail = _path_splitroot(first, False)
        else:
            drive, root, tail = state
        del state
        for index in range(resume, len(rest)):
            if index != resume or b is None:
                b = fspath(rest[index])
            p_drive, p_root, p_tail = _path_splitroot(b, False)
            if p_root:
                if p_drive or not drive:
                    drive = p_drive
                root = p_root
                tail = p_tail
                continue
            elif p_drive and p_drive != drive:
                if p_drive.lower() != drive.lower():
                    drive, root, tail = p_drive, p_root, p_tail
                    continue
                drive = p_drive
            if tail and tail[-1] not in seps:
                tail = tail + slash
            tail = tail + p_tail
        if tail and not root and drive and drive[-1:] not in colon + seps:
            return drive + slash + tail
        return drive + root + tail
    except (TypeError, AttributeError, BytesWarning):
        # Inspect originals, as CPython does; never repeat __fspath__ calls.
        has_str = has_bytes = False
        for value in (first, *rest):
            if isinstance(value, str):
                has_str = True
            elif isinstance(value, bytes):
                has_bytes = True
            else:
                raise TypeError(
                    "join() argument must be str, bytes, or os.PathLike object, "
                    f"not {value.__class__.__name__!r}"
                ) from None
        if has_str and has_bytes:
            raise TypeError(
                "Can't mix strings and bytes in path components"
            ) from None
        raise


class _Path:
    sep = sep
    pathsep = pathsep
    curdir = curdir
    pardir = pardir
    extsep = extsep
    altsep = altsep

    @staticmethod
    def join(first: Any, *rest: Any) -> Any:
        return _path_join(first, rest, sep == "/")

    @staticmethod
    def isabs(path: str) -> bool:
        return bool(_MOLT_PATH_ISABS(path))

    @staticmethod
    def dirname(path: Any) -> str | bytes:
        if name == "nt" and type(path) is not str:
            return _path_nt_split(path)[0]
        return _expect_str(_MOLT_PATH_DIRNAME(path), "path_dirname")

    @staticmethod
    def basename(path: Any) -> str | bytes:
        if name == "nt" and type(path) is not str:
            return _path_nt_split(path)[1]
        return _expect_str(_MOLT_PATH_BASENAME(path), "path_basename")

    @staticmethod
    def split(path: Any) -> tuple[str, str] | tuple[bytes, bytes]:
        if name == "nt" and type(path) is not str:
            return _path_nt_split(path)
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
    def expandvars(path: Any) -> str | bytes:
        return _path_expandvars(path)

    @staticmethod
    def abspath(path: str) -> str:
        return _expect_str(_MOLT_PATH_ABSPATH(path), "path_abspath")

    @staticmethod
    def relpath(path: str, start: str | None = None) -> str:
        return _expect_str(_MOLT_PATH_RELPATH(path, start), "path_relpath")

    @staticmethod
    def expanduser(path: Any) -> str | bytes:
        return _path_expanduser(path)

    @staticmethod
    def exists(path: Any) -> bool:
        return bool(_MOLT_PATH_EXISTS(path))

    @staticmethod
    def isdir(path: Any) -> bool:
        return bool(_MOLT_PATH_ISDIR(path))

    @staticmethod
    def isfile(path: Any) -> bool:
        return bool(_MOLT_PATH_ISFILE(path))

    @staticmethod
    def islink(path: Any) -> bool:
        return bool(_MOLT_PATH_ISLINK(path))

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
    def commonpath(paths: Any) -> str | bytes:
        _require_cap("fs.read")
        intrinsic = _require_callable_intrinsic(
            _MOLT_OS_PATH_COMMONPATH, "molt_os_path_commonpath"
        )
        return intrinsic(list(paths), _WINDOWS_UTF16_ROOT)

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
        return float(intrinsic(path))

    @staticmethod
    def getctime(path: Any) -> float:
        _require_cap("fs.read")
        intrinsic = _require_callable_intrinsic(
            _MOLT_OS_PATH_GETCTIME, "molt_os_path_getctime"
        )
        return float(intrinsic(path))

    @staticmethod
    def getmtime(path: Any) -> float:
        _require_cap("fs.read")
        intrinsic = _require_callable_intrinsic(
            _MOLT_OS_PATH_GETMTIME, "molt_os_path_getmtime"
        )
        return float(intrinsic(path))

    @staticmethod
    def getsize(path: Any) -> int:
        _require_cap("fs.read")
        intrinsic = _require_callable_intrinsic(
            _MOLT_OS_PATH_GETSIZE, "molt_os_path_getsize"
        )
        return int(intrinsic(path))

    @staticmethod
    def samefile(path1: Any, path2: Any) -> bool:
        _require_cap("fs.read")
        intrinsic = _require_callable_intrinsic(
            _MOLT_OS_PATH_SAMEFILE, "molt_os_path_samefile"
        )
        return bool(intrinsic(path1, path2))

    # Lib/genericpath.py: the same (st_ino, st_dev) pair names the same file.
    @staticmethod
    def samestat(s1: Any, s2: Any) -> bool:
        return s1.st_ino == s2.st_ino and s1.st_dev == s2.st_dev

    @staticmethod
    def sameopenfile(fp1: Any, fp2: Any) -> bool:
        return _Path.samestat(fstat(fp1), fstat(fp2))

    @staticmethod
    def realpath(path: Any, *, strict: bool = False) -> str | bytes:
        return _MOLT_OS_PATH_REALPATH(path)


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


def listdir(path: Any = ".") -> list[str] | list[bytes]:
    _require_cap("fs.read")
    return _MOLT_OS_LISTDIR_V2(path)


_MOLT_ENV_MAPPING_INIT()
environ = _createenviron()
del _createenviron
supports_bytes_environ = name != "nt"
__all__.append("supports_bytes_environ")
if supports_bytes_environ:

    def _check_bytes(value):
        if not isinstance(value, bytes):
            raise TypeError("bytes expected, not %s" % type(value).__name__)
        return value

    environb = _Environ(_check_bytes, bytes, _check_bytes, bytes)
    del _check_bytes

    def getenvb(key, default=None):
        return environb.get(key, default)

    __all__.extend(("environb", "getenvb"))


def getpid() -> int:
    return int(_MOLT_OS_GETPID_V2())


def urandom(n: Any) -> bytes:
    # The intrinsic converts `size` (Py_ssize_t), then enforces the canonical
    # `random.entropy` operation, so os.urandom needs the `random` grant only.
    intrinsic = _require_callable_intrinsic(_MOLT_OS_URANDOM, "molt_os_urandom")
    return intrinsic(n)


def getcwd() -> str:
    _require_cap("fs.read")
    return str(_MOLT_OS_GETCWD_V2())


def getenv(key: str, default: Any = None) -> Any:
    return environ.get(key, default)


def putenv(key: Any, value: Any) -> None:
    _MOLT_REQUIRE_OPERATION("env.putenv")
    _MOLT_ENV_PUTENV(key, value)


def unsetenv(key: Any) -> None:
    _MOLT_REQUIRE_OPERATION("env.unsetenv")
    _MOLT_ENV_UNSETENV(key)


def unlink(path: Any) -> None:
    _Path.unlink(path)


def remove(path: Any) -> None:
    unlink(path)


def readlink(path: Any, *, dir_fd: int | None = None) -> str | bytes:
    if dir_fd is not None:
        return _MOLT_OS_READLINK_AT(path, dir_fd)
    return _MOLT_PATH_READLINK(path)


def symlink(
    src: Any,
    dst: Any,
    target_is_directory: bool = False,
    *,
    dir_fd: int | None = None,
) -> None:
    _require_cap("fs.write")
    if dir_fd is not None:
        _MOLT_OS_SYMLINK_AT(src, dst, dir_fd)
        return
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
    _MOLT_OS_MAKEDIRS(fspath(name), mode, bool(exist_ok))


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
    return _MOLT_OS_LSEEK(fd, pos, how)


def ftruncate(fd: int, length: int) -> None:
    _MOLT_OS_FTRUNCATE(fd, length)


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
    if dir_fd is not None:
        return _MOLT_OS_STAT_AT(path, dir_fd, bool(follow_symlinks))
    if bool(follow_symlinks):
        return _MOLT_OS_STAT(path)
    return _MOLT_OS_LSTAT(path)


def lstat(path: Any, *, dir_fd: int | None = None) -> stat_result:
    if dir_fd is not None:
        return _MOLT_OS_LSTAT_AT(path, dir_fd)
    return _MOLT_OS_LSTAT(path)


def fstat(fd: int) -> stat_result:
    return _MOLT_OS_FSTAT(fd)


def rename(
    src: Any,
    dst: Any,
    *,
    src_dir_fd: int | None = None,
    dst_dir_fd: int | None = None,
) -> None:
    _require_cap("fs.write")
    if src_dir_fd is not None or dst_dir_fd is not None:
        _MOLT_OS_RENAME_AT(src, src_dir_fd, dst, dst_dir_fd)
        return
    intrinsic = _require_callable_intrinsic(
        _require_intrinsic("molt_os_rename"), "molt_os_rename"
    )
    intrinsic(src, dst)


def replace(
    src: Any,
    dst: Any,
    *,
    src_dir_fd: int | None = None,
    dst_dir_fd: int | None = None,
) -> None:
    _require_cap("fs.write")
    if src_dir_fd is not None or dst_dir_fd is not None:
        _MOLT_OS_REPLACE_AT(src, src_dir_fd, dst, dst_dir_fd)
        return
    intrinsic = _require_callable_intrinsic(
        _require_intrinsic("molt_os_replace"), "molt_os_replace"
    )
    intrinsic(src, dst)


def access(path: Any, mode: int, *, follow_symlinks: bool = True) -> bool:
    _require_cap("fs.read")
    intrinsic = _require_callable_intrinsic(_MOLT_OS_ACCESS, "molt_os_access")
    return bool(intrinsic(path, mode))


def chdir(path: Any) -> None:
    _require_cap("fs.write")
    intrinsic = _require_callable_intrinsic(_MOLT_OS_CHDIR, "molt_os_chdir")
    intrinsic(path)


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
    if src_dir_fd is not None or dst_dir_fd is not None or not bool(follow_symlinks):
        _MOLT_OS_LINK_AT(src, src_dir_fd, dst, dst_dir_fd, bool(follow_symlinks))
        return
    intrinsic = _require_callable_intrinsic(_MOLT_OS_LINK, "molt_os_link")
    intrinsic(src, dst)


def truncate(path: Any, length: int) -> None:
    _require_cap("fs.write")
    intrinsic = _require_callable_intrinsic(_MOLT_OS_TRUNCATE, "molt_os_truncate")
    intrinsic(path, length)


def umask(mask: int) -> int:
    intrinsic = _require_callable_intrinsic(_MOLT_OS_UMASK, "molt_os_umask")
    return int(intrinsic(mask))


def uname() -> uname_result:
    intrinsic = _require_callable_intrinsic(_MOLT_OS_UNAME, "molt_os_uname")
    return _struct_sequence_new(_UNAME_RESULT, intrinsic())


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


def _system_status_from_returncode(returncode: int) -> int:
    code = int(returncode)
    if name == "nt":
        return code
    if code < 0:
        return -code
    return code << 8


def _system_shell_argv(command: str | bytes) -> list[str | bytes]:
    if name == "nt":
        return ["cmd.exe", "/c", command]
    return ["/bin/sh", "-c", command]


def _system_type_name(value: Any) -> str:
    if value is None:
        return "None"
    return type(value).__name__


def system(command: Any) -> int:
    # CPython's argument clinic declares `command: Py_UNICODE` on Windows and
    # `command: FSConverter` everywhere else (Modules/posixmodule.c).
    if name == "nt":
        if not isinstance(command, str):
            raise TypeError(
                "system() argument 'command' must be str, "
                f"not {_system_type_name(command)}"
            )
        if "\x00" in command:
            raise ValueError("embedded null character")
    else:
        # PyUnicode_FSConverter: os.PathLike protocol, filesystem encoding,
        # then the embedded-NUL check on the encoded bytes.
        command = fsencode(command)
        if b"\x00" in command:
            raise ValueError("embedded null byte")
    raw = _MOLT_SUBPROCESS_RUN(
        _system_shell_argv(command),
        False,
        None,
        False,
        None,
        None,
        None,
    )
    return _system_status_from_returncode(int(raw[0]))


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


if _sysconf_names_raw is not None:

    def sysconf(name: int | str) -> int:
        if isinstance(name, str):
            _name = sysconf_names.get(name, -1)
            if _name == -1:
                raise ValueError("unrecognized configuration name")
            name = _name
        return int(_MOLT_OS_SYSCONF(int(name)))


del _sysconf_names_raw


# POSIX `time_t` on every platform molt targets is a signed 64-bit integer;
# a seconds component beyond this overflows the kernel `timespec` and is an
# OverflowError in CPython.
_TIME_T_MAX = (1 << 63) - 1
_TIME_T_MIN = -(1 << 63)


def _check_time_t(sec: int) -> int:
    if sec > _TIME_T_MAX or sec < _TIME_T_MIN:
        raise OverflowError("timestamp out of range for platform time_t")
    return sec


def _seconds_float_to_sec_nsec(t: float) -> tuple[int, int]:
    """Split float seconds into integer `(sec, nsec)` for a POSIX `timespec`.

    Both components stay small (`sec` ~ epoch scale, `nsec` < 1e9), so neither
    exceeds the inline-int range when marshalled to the `*at` intrinsic. The
    sub-second remainder is rounded and carried across the second boundary so
    the pair is always a normalized timespec (0 <= nsec < 1e9).
    """
    sec = int(t)
    frac_ns = int(round((t - sec) * 1_000_000_000.0))
    if frac_ns >= 1_000_000_000:
        sec += 1
        frac_ns -= 1_000_000_000
    elif frac_ns < 0:
        sec -= 1
        frac_ns += 1_000_000_000
    return _check_time_t(sec), frac_ns


def _ns_to_sec_nsec(value: int) -> tuple[int, int]:
    """Split an integer nanosecond count into a normalized `(sec, nsec)` pair."""
    value = int(value)
    sec = value // 1_000_000_000
    nsec = value - sec * 1_000_000_000
    return _check_time_t(sec), nsec


def utime(
    path: Any,
    times: tuple[float, float] | None = None,
    *,
    ns: tuple[int, int] | None = None,
    dir_fd: int | None = None,
    follow_symlinks: bool = True,
) -> None:
    if times is not None and ns is not None:
        raise ValueError("utime: you may specify either 'times' or 'ns' but not both")
    _require_cap("fs.write")
    # The base `molt_os_utime` already maps `times=None` -> current time via a
    # null timeval pair, so keep its fast path when no dir_fd-relative behavior
    # and no explicit `ns=` is requested.
    if dir_fd is None and ns is None and bool(follow_symlinks):
        if times is not None:
            if len(times) != 2:
                raise TypeError(
                    "utime: 'times' must be either a tuple of two ints or None"
                )
            _MOLT_OS_UTIME(path, float(times[0]), float(times[1]))
        else:
            _MOLT_OS_UTIME(path, None, None)
        return
    # dir_fd / ns=... / follow_symlinks=False -> the `*at` intrinsic. Times are
    # pre-split into (sec, nsec) pairs so no argument exceeds the inline-int
    # range; a nsec of -1 selects "current time" (POSIX UTIME_NOW) for that
    # timestamp.
    if ns is not None:
        if len(ns) != 2:
            raise TypeError("utime: 'ns' must be a tuple of two ints")
        atime_sec, atime_nsec = _ns_to_sec_nsec(ns[0])
        mtime_sec, mtime_nsec = _ns_to_sec_nsec(ns[1])
    elif times is not None:
        if len(times) != 2:
            raise TypeError("utime: 'times' must be either a tuple of two ints or None")
        atime_sec, atime_nsec = _seconds_float_to_sec_nsec(float(times[0]))
        mtime_sec, mtime_nsec = _seconds_float_to_sec_nsec(float(times[1]))
    else:
        atime_sec, atime_nsec = 0, -1
        mtime_sec, mtime_nsec = 0, -1
    _MOLT_OS_UTIME_AT(
        path,
        dir_fd,
        atime_sec,
        atime_nsec,
        mtime_sec,
        mtime_nsec,
        bool(follow_symlinks),
    )


def sendfile(out_fd: int, in_fd: int, offset: int | None, count: int) -> int:
    return int(_MOLT_OS_SENDFILE(int(out_fd), int(in_fd), offset, int(count)))


def removedirs(name: Any) -> None:
    _require_cap("fs.write")
    intrinsic = _require_callable_intrinsic(_MOLT_OS_REMOVEDIRS, "molt_os_removedirs")
    intrinsic(name)


def get_terminal_size(fd: int = 1) -> terminal_size:
    intrinsic = _require_callable_intrinsic(
        _MOLT_OS_GET_TERMINAL_SIZE, "molt_os_get_terminal_size"
    )
    return _struct_sequence_new(_TERMINAL_SIZE, intrinsic(fd))


def walk(
    top: Any,
    topdown: bool = True,
    onerror: Any = None,
    followlinks: bool = False,
) -> Iterator[
    tuple[str, list[str], list[str]] | tuple[bytes, list[bytes], list[bytes]]
]:
    _require_cap("fs.read")
    intrinsic = _require_callable_intrinsic(_MOLT_OS_WALK, "molt_os_walk")
    yield from intrinsic(top, bool(topdown), bool(followlinks))


def scandir(path: Any = ".") -> list[DirEntry]:
    _require_cap("fs.read")
    intrinsic = _require_callable_intrinsic(_MOLT_OS_SCANDIR, "molt_os_scandir")
    return [
        DirEntry(name, entry_path, is_dir, is_file, is_symlink)
        for name, entry_path, is_dir, is_file, is_symlink in intrinsic(path)
    ]


# ---------------------------------------------------------------------------
# Capability-introspection sets. Membership mirrors CPython exactly for the
# functions molt implements: each set holds the *function objects* CPython's
# Lib/os.py registers by C-level capability name (`HAVE_FSTATAT` -> stat,
# `HAVE_RENAMEAT` -> rename, ...). `os.lstat` and `os.replace` accept dir_fd
# keywords but are never registered: CPython names only `stat` and `rename`.
# ---------------------------------------------------------------------------
supports_dir_fd = {
    stat,
    rename,
    link,
    symlink,
    readlink,
    utime,
}

supports_follow_symlinks = {
    stat,
    link,
    utime,
}


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
