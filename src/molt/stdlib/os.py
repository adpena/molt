"""Minimal os shim for Molt."""

from __future__ import annotations

import abc as _abc

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


_MOLT_ENV_GET = _require_intrinsic("molt_env_get", globals())
_MOLT_ENV_SNAPSHOT = _require_intrinsic("molt_env_snapshot", globals())
_MOLT_ENV_SET = _require_intrinsic("molt_env_set", globals())
_MOLT_ENV_UNSET = _require_intrinsic("molt_env_unset", globals())
_MOLT_ENV_LEN = _require_intrinsic("molt_env_len", globals())
_MOLT_ENV_CONTAINS = _require_intrinsic("molt_env_contains", globals())
_MOLT_ENV_POPITEM = _require_intrinsic("molt_env_popitem", globals())
_MOLT_ENV_CLEAR = _require_intrinsic("molt_env_clear", globals())
_MOLT_ENV_PUTENV = _require_intrinsic("molt_env_putenv", globals())
_MOLT_ENV_UNSETENV = _require_intrinsic("molt_env_unsetenv", globals())
_MOLT_OS_NAME = _require_intrinsic("molt_os_name", globals())
_MOLT_PATH_EXISTS = _require_intrinsic("molt_path_exists", globals())
_MOLT_PATH_LISTDIR = _require_intrinsic("molt_path_listdir", globals())
_MOLT_PATH_MKDIR = _require_intrinsic("molt_path_mkdir", globals())
_MOLT_PATH_UNLINK = _require_intrinsic("molt_path_unlink", globals())
_MOLT_PATH_RMDIR = _require_intrinsic("molt_path_rmdir", globals())
_MOLT_PATH_CHMOD = _require_intrinsic("molt_path_chmod", globals())
_MOLT_GETCWD = _require_intrinsic("molt_getcwd", globals())
_MOLT_GETPID = _require_intrinsic("molt_getpid", globals())
_MOLT_PATH_JOIN_MANY = _require_intrinsic("molt_path_join_many", globals())
_MOLT_PATH_ISABS = _require_intrinsic("molt_path_isabs", globals())
_MOLT_PATH_DIRNAME = _require_intrinsic("molt_path_dirname", globals())
_MOLT_PATH_BASENAME = _require_intrinsic("molt_path_basename", globals())
_MOLT_PATH_SPLIT = _require_intrinsic("molt_path_split", globals())
_MOLT_PATH_SPLITEXT = _require_intrinsic("molt_path_splitext", globals())
_MOLT_PATH_NORMPATH = _require_intrinsic("molt_path_normpath", globals())
_MOLT_PATH_ABSPATH = _require_intrinsic("molt_path_abspath", globals())
_MOLT_PATH_RELPATH = _require_intrinsic("molt_path_relpath", globals())
_MOLT_PATH_EXPANDVARS_ENV = _require_intrinsic("molt_path_expandvars_env", globals())
_MOLT_PATH_MAKEDIRS = _require_intrinsic("molt_path_makedirs", globals())
_MOLT_PATH_ISDIR = _require_intrinsic("molt_path_isdir", globals())
_MOLT_PATH_ISFILE = _require_intrinsic("molt_path_isfile", globals())
_MOLT_PATH_ISLINK = _require_intrinsic("molt_path_islink", globals())
_MOLT_PATH_READLINK = _require_intrinsic("molt_path_readlink", globals())
_MOLT_PATH_SYMLINK = _require_intrinsic("molt_path_symlink", globals())
_MOLT_OS_CLOSE = _require_intrinsic("molt_os_close", globals())
_MOLT_OS_READ = _require_intrinsic("molt_os_read", globals())
_MOLT_OS_WRITE = _require_intrinsic("molt_os_write", globals())
_MOLT_OS_PIPE = _require_intrinsic("molt_os_pipe", globals())
_MOLT_OS_DUP = _require_intrinsic("molt_os_dup", globals())
_MOLT_OS_GET_INHERITABLE = _require_intrinsic("molt_os_get_inheritable", globals())
_MOLT_OS_SET_INHERITABLE = _require_intrinsic("molt_os_set_inheritable", globals())
_MOLT_OS_URANDOM = _require_intrinsic("molt_os_urandom", globals())
_MOLT_OS_FSENCODE = _require_intrinsic("molt_os_fsencode", globals())
_MOLT_CAP_REQUIRE = _require_intrinsic("molt_capabilities_require", globals())


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
    "SEEK_SET",
    "SEEK_CUR",
    "SEEK_END",
    "getcwd",
    "getpid",
    "getenv",
    "putenv",
    "unsetenv",
    "urandom",
    "listdir",
    "mkdir",
    "chmod",
    "makedirs",
    "rmdir",
    "read",
    "write",
    "close",
    "pipe",
    "dup",
    "get_inheritable",
    "set_inheritable",
    "unlink",
    "remove",
    "readlink",
    "symlink",
    "environ",
    "path",
    "PathLike",
    "fspath",
    "fsencode",
]

name = _resolve_os_name()
if name == "nt":
    sep = "\\"
    pathsep = ";"
    linesep = "\r\n"
    altsep = "/"
else:
    sep = "/"
    pathsep = ":"
    linesep = "\n"
    altsep = None
curdir = "."
pardir = ".."
extsep = "."
SEEK_SET = 0
SEEK_CUR = 1
SEEK_END = 2

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
    raw = _MOLT_ENV_SNAPSHOT()
    if not isinstance(raw, dict):
        raise RuntimeError("os env snapshot intrinsic returned invalid value")
    out: dict[str, str] = {}
    for key, value in raw.items():
        if not isinstance(key, str) or not isinstance(value, str):
            raise RuntimeError("os env snapshot intrinsic returned invalid value")
        out[key] = value
    return out


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
    if isinstance(path, (str, bytes)):
        return path
    method = getattr(path, "__fspath__", None)
    if method is None:
        raise TypeError(
            f"expected str, bytes or os.PathLike object, not {type(path).__name__}"
        )
    value = method()
    if isinstance(value, (str, bytes)):
        return value
    raise TypeError(
        f"expected str, bytes or os.PathLike object, not {type(path).__name__}"
    )


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
        first: str,
        second: str | None = None,
        third: str | None = None,
        fourth: str | None = None,
    ) -> str:
        parts: list[str] = []
        if first:
            parts.append(first)
        if second:
            parts.append(second)
        if third:
            parts.append(third)
        if fourth:
            parts.append(fourth)
        if not parts:
            return ""
        return _expect_str(
            _MOLT_PATH_JOIN_MANY(parts[0], tuple(parts[1:])), "path_join_many"
        )

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
        env = environ.copy()
        return _expect_str(_MOLT_PATH_EXPANDVARS_ENV(path, env), "path_expandvars_env")

    @staticmethod
    def abspath(path: str) -> str:
        return _expect_str(_MOLT_PATH_ABSPATH(path), "path_abspath")

    @staticmethod
    def relpath(path: str, start: str | None = None) -> str:
        return _expect_str(_MOLT_PATH_RELPATH(path, start), "path_relpath")

    @staticmethod
    def exists(path: Any) -> bool:
        _require_cap("fs.read")
        intrinsic = _require_callable_intrinsic(_MOLT_PATH_EXISTS, "molt_path_exists")
        return bool(intrinsic(path))

    @staticmethod
    def isdir(path: Any) -> bool:
        _require_cap("fs.read")
        intrinsic = _require_callable_intrinsic(_MOLT_PATH_ISDIR, "molt_path_isdir")
        return bool(intrinsic(path))

    @staticmethod
    def isfile(path: Any) -> bool:
        _require_cap("fs.read")
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
        intrinsic = _require_callable_intrinsic(_MOLT_PATH_RMDIR, "molt_path_rmdir")
        intrinsic(path)


path = _Path()


def listdir(path: Any = ".") -> list[str]:
    _require_cap("fs.read")
    intrinsic = _require_callable_intrinsic(_MOLT_PATH_LISTDIR, "molt_path_listdir")
    res = intrinsic(path)
    if not isinstance(res, list):
        raise RuntimeError("os listdir intrinsic returned invalid value")
    if not all(isinstance(entry, str) for entry in res):
        raise RuntimeError("os listdir intrinsic returned invalid value")
    return list(res)


environ = _Environ()


def getpid() -> int:
    intrinsic = _require_callable_intrinsic(_MOLT_GETPID, "molt_getpid")
    return int(intrinsic())


def urandom(n: Any) -> bytes:
    _require_cap("rand")
    intrinsic = _require_callable_intrinsic(_MOLT_OS_URANDOM, "molt_os_urandom")
    return intrinsic(n)


def getcwd() -> str:
    _require_cap("fs.read")
    intrinsic = _require_callable_intrinsic(_MOLT_GETCWD, "molt_getcwd")
    return intrinsic()


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
    intrinsic = _require_callable_intrinsic(_MOLT_PATH_READLINK, "molt_path_readlink")
    return _expect_str(intrinsic(path), "path_readlink")


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
    intrinsic = _require_callable_intrinsic(_MOLT_PATH_SYMLINK, "molt_path_symlink")
    intrinsic(src, dst, bool(target_is_directory))


def rmdir(path: Any) -> None:
    _Path.rmdir(path)


def mkdir(path: Any, mode: int = 0o777) -> None:
    _require_cap("fs.write")
    intrinsic = _require_callable_intrinsic(_MOLT_PATH_MKDIR, "molt_path_mkdir")
    intrinsic(path)


def chmod(path: Any, mode: int) -> None:
    _require_cap("fs.write")
    intrinsic = _require_callable_intrinsic(_MOLT_PATH_CHMOD, "molt_path_chmod")
    intrinsic(path, mode)


def makedirs(name: Any, mode: int = 0o777, exist_ok: bool = False) -> None:
    del mode
    path = fspath(name)
    intrinsic = _require_callable_intrinsic(_MOLT_PATH_MAKEDIRS, "molt_path_makedirs")
    intrinsic(path, bool(exist_ok))


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
