"""Minimal os shim for Molt."""

from __future__ import annotations

from collections.abc import ItemsView, KeysView, ValuesView
from _intrinsics import require_intrinsic as _require_intrinsic

TYPE_CHECKING = False

if TYPE_CHECKING:
    from typing import Any, Iterator, MutableMapping
else:

    class _TypingAlias:
        __slots__ = ()

        def __getitem__(self, _item):
            return self

    Any = object
    Iterator = _TypingAlias()
    MutableMapping = _TypingAlias()


_MOLT_ENV_GET = _require_intrinsic("molt_env_get", globals())
_MOLT_OS_NAME = _require_intrinsic("molt_os_name", globals())
_MOLT_PATH_EXISTS = _require_intrinsic("molt_path_exists", globals())
_MOLT_PATH_LISTDIR = _require_intrinsic("molt_path_listdir", globals())
_MOLT_PATH_MKDIR = _require_intrinsic("molt_path_mkdir", globals())
_MOLT_PATH_UNLINK = _require_intrinsic("molt_path_unlink", globals())
_MOLT_PATH_RMDIR = _require_intrinsic("molt_path_rmdir", globals())
_MOLT_PATH_CHMOD = _require_intrinsic("molt_path_chmod", globals())
_MOLT_GETCWD = _require_intrinsic("molt_getcwd", globals())
_MOLT_GETPID = _require_intrinsic("molt_getpid", globals())
_MOLT_PATH_JOIN = _require_intrinsic("molt_path_join", globals())
_MOLT_PATH_ISABS = _require_intrinsic("molt_path_isabs", globals())
_MOLT_PATH_DIRNAME = _require_intrinsic("molt_path_dirname", globals())
_MOLT_PATH_SPLITEXT = _require_intrinsic("molt_path_splitext", globals())
_MOLT_PATH_NORMPATH = _require_intrinsic("molt_path_normpath", globals())
_MOLT_PATH_ABSPATH = _require_intrinsic("molt_path_abspath", globals())
_MOLT_PATH_PARTS = _require_intrinsic("molt_path_parts", globals())
_MOLT_OS_CLOSE = _require_intrinsic("molt_os_close", globals())
_MOLT_OS_DUP = _require_intrinsic("molt_os_dup", globals())
_MOLT_OS_GET_INHERITABLE = _require_intrinsic("molt_os_get_inheritable", globals())
_MOLT_OS_SET_INHERITABLE = _require_intrinsic("molt_os_set_inheritable", globals())
_MOLT_OS_URANDOM = _require_intrinsic("molt_os_urandom", globals())
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
    "urandom",
    "listdir",
    "mkdir",
    "chmod",
    "makedirs",
    "rmdir",
    "close",
    "dup",
    "get_inheritable",
    "set_inheritable",
    "unlink",
    "remove",
    "environ",
    "path",
    "PathLike",
    "fspath",
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

_ENV_STORE: dict[str, str] = {}
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
    if key in _ENV_STORE:
        return _ENV_STORE[key]
    return _MOLT_ENV_GET(key, default)


def _require_cap(name: str) -> None:
    _MOLT_CAP_REQUIRE(name)
    return None


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
    def __init__(self) -> None:
        self._store = _ENV_STORE

    def _backend(self) -> MutableMapping[str, str]:
        return self._store

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
        return self._backend()[key]

    def __setitem__(self, key: str, value: str) -> None:
        _require_cap("env.write")
        key = self._check_key(key)
        value = self._check_value(value)
        backend = self._backend()
        backend[key] = value
        if backend is not self._store:
            self._store[key] = value

    def __delitem__(self, key: str) -> None:
        _require_cap("env.write")
        key = self._check_key(key)
        backend = self._backend()
        if backend is self._store:
            backend.pop(key)
            return
        removed = False
        if key in self._store:
            del self._store[key]
            removed = True
        try:
            backend.pop(key)
            removed = True
        except KeyError:
            if not removed:
                raise

    def __iter__(self) -> Iterator[str]:
        _require_cap("env.read")
        return iter(self._backend())

    def __len__(self) -> int:
        _require_cap("env.read")
        return len(self._backend())

    def __contains__(self, key: object) -> bool:
        _require_cap("env.read")
        key = self._check_key(key)
        value = _molt_env_get(key, _ENV_MISSING)
        if value is not _ENV_MISSING:
            return True
        return key in self._backend()

    def __repr__(self) -> str:
        return "environ(" + repr(self.copy()) + ")"

    def copy(self) -> dict[str, str]:
        _require_cap("env.read")
        return dict(self)

    def get(self, key: str, default: Any = None) -> Any:
        _require_cap("env.read")
        key = self._check_key(key)
        value = _molt_env_get(key, _ENV_MISSING)
        if value is not _ENV_MISSING:
            return value
        return self._backend().get(key, default)

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
        backend = self._backend()
        if backend is self._store:
            if default is _ENV_MISSING:
                return backend.pop(key)
            return backend.pop(key, default)
        if key in self._store:
            value = self._store.pop(key)
            backend.pop(key, None)
            return value
        if default is _ENV_MISSING:
            return backend.pop(key)
        return backend.pop(key, default)

    def popitem(self) -> tuple[str, str]:
        _require_cap("env.write")
        backend = self._backend()
        if backend is self._store:
            return backend.popitem()
        key, value = backend.popitem()
        self._store.pop(key, None)
        return key, value

    def clear(self) -> None:
        _require_cap("env.write")
        backend = self._backend()
        backend.clear()
        if backend is not self._store:
            self._store.clear()

    def items(self) -> ItemsView[str, str]:
        _require_cap("env.read")
        return self._backend().items()

    def keys(self) -> KeysView[str]:
        _require_cap("env.read")
        return self._backend().keys()

    def values(self) -> ValuesView[str]:
        _require_cap("env.read")
        return self._backend().values()


class PathLike:
    __slots__ = ()

    def __fspath__(self) -> str | bytes:
        raise NotImplementedError


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
        path = parts[0]
        for part in parts[1:]:
            path = _expect_str(_MOLT_PATH_JOIN(path, part), "path_join")
        return path

    @staticmethod
    def isabs(path: str) -> bool:
        return bool(_MOLT_PATH_ISABS(path))

    @staticmethod
    def dirname(path: str) -> str:
        return _expect_str(_MOLT_PATH_DIRNAME(path), "path_dirname")

    @staticmethod
    def basename(path: str) -> str:
        if not path:
            return ""
        parts = _MOLT_PATH_PARTS(path)
        if not isinstance(parts, list):
            raise RuntimeError("os path_parts intrinsic returned invalid value")
        if not parts:
            normalized = _expect_str(_MOLT_PATH_NORMPATH(path), "path_normpath")
            if normalized in (".", ".."):
                return normalized
            return ""
        tail = parts[-1]
        if isinstance(tail, str):
            return tail
        raise RuntimeError("os path_parts intrinsic returned invalid value")

    @staticmethod
    def split(path: str) -> tuple[str, str]:
        return _Path.dirname(path), _Path.basename(path)

    @staticmethod
    def splitext(path: str) -> tuple[str, str]:
        return _expect_splitext(_MOLT_PATH_SPLITEXT(path))

    @staticmethod
    def normpath(path: str) -> str:
        return _expect_str(_MOLT_PATH_NORMPATH(path), "path_normpath")

    @staticmethod
    def expandvars(path: str) -> str:
        _require_cap("env.read")
        if not path or "$" not in path:
            return path

        def _is_var_char(ch: str) -> bool:
            if "a" <= ch <= "z" or "A" <= ch <= "Z":
                return True
            if "0" <= ch <= "9":
                return True
            return ch == "_"

        out: list[str] = []
        idx = 0
        length = len(path)
        while idx < length:
            ch = path[idx]
            if ch != "$":
                out.append(ch)
                idx += 1
                continue
            if idx + 1 >= length:
                out.append(ch)
                idx += 1
                continue
            next_ch = path[idx + 1]
            if next_ch == "{":
                end = path.find("}", idx + 2)
                if end == -1:
                    out.append(path[idx:])
                    break
                name = path[idx + 2 : end]
                if not name:
                    out.append(path[idx : end + 1])
                else:
                    value = _molt_env_get(name, None)
                    if value is None:
                        out.append(path[idx : end + 1])
                    else:
                        out.append(str(value))
                idx = end + 1
                continue
            if next_ch == "$":
                out.append("$$")
                idx += 2
                continue
            start = idx + 1
            end = start
            while end < length:
                ch = path[end]
                if _is_var_char(ch):
                    end += 1
                    continue
                break
            if end == start:
                out.append("$")
                idx += 1
                continue
            name = path[start:end]
            value = _molt_env_get(name, None)
            if value is None:
                out.append(path[idx:end])
            else:
                out.append(str(value))
            idx = end
        return "".join(out)

    @staticmethod
    def abspath(path: str) -> str:
        return _expect_str(_MOLT_PATH_ABSPATH(path), "path_abspath")

    @staticmethod
    def relpath(path: str, start: str | None = None) -> str:
        if path == "":
            raise ValueError("no path specified")
        if start is None:
            start = curdir
        start_abs = _Path.abspath(start)
        path_abs = _Path.abspath(path)
        start_parts = [part for part in start_abs.split(sep) if part]
        path_parts = [part for part in path_abs.split(sep) if part]
        common = 0
        limit = min(len(start_parts), len(path_parts))
        while common < limit and start_parts[common] == path_parts[common]:
            common += 1
        rel_parts = [pardir] * (len(start_parts) - common)
        rel_parts.extend(path_parts[common:])
        if not rel_parts:
            return curdir
        return sep.join(rel_parts)

    @staticmethod
    def exists(path: Any) -> bool:
        _require_cap("fs.read")
        intrinsic = _require_intrinsic("_molt_path_exists", _MOLT_PATH_EXISTS)
        res = intrinsic(path)
        if res is None:
            return False
        return res

    @staticmethod
    def isdir(path: Any) -> bool:
        _require_cap("fs.read")
        intrinsic = _require_intrinsic("_molt_path_listdir", _MOLT_PATH_LISTDIR)
        try:
            intrinsic(path)
            return True
        except Exception:
            return False

    @staticmethod
    def isfile(path: Any) -> bool:
        _require_cap("fs.read")
        intrinsic = _require_intrinsic("_molt_path_listdir", _MOLT_PATH_LISTDIR)
        try:
            intrinsic(path)
            return False
        except FileNotFoundError:
            return False
        except Exception:
            return True

    @staticmethod
    def unlink(path: Any) -> None:
        _require_cap("fs.write")
        intrinsic = _require_intrinsic("_molt_path_unlink", _MOLT_PATH_UNLINK)
        intrinsic(path)

    @staticmethod
    def rmdir(path: Any) -> None:
        _require_cap("fs.write")
        intrinsic = _require_intrinsic("_molt_path_rmdir", _MOLT_PATH_RMDIR)
        intrinsic(path)


path = _Path()


def listdir(path: Any = ".") -> list[str]:
    _require_cap("fs.read")
    intrinsic = _require_intrinsic("_molt_path_listdir", _MOLT_PATH_LISTDIR)
    res = intrinsic(path)
    if isinstance(res, list):
        return res
    raise FileNotFoundError(path)


environ = _Environ()


def getpid() -> int:
    intrinsic = _require_intrinsic("_molt_getpid", _MOLT_GETPID)
    return int(intrinsic())


def urandom(n: Any) -> bytes:
    _require_cap("rand")
    intrinsic = _require_intrinsic("_molt_os_urandom", _MOLT_OS_URANDOM)
    return intrinsic(n)


def getcwd() -> str:
    _require_cap("fs.read")
    intrinsic = _require_intrinsic("_molt_getcwd", _MOLT_GETCWD)
    return intrinsic()


def getenv(key: str, default: Any = None) -> Any:
    _require_cap("env.read")
    return _molt_env_get(key, default)


def unlink(path: Any) -> None:
    _Path.unlink(path)


def remove(path: Any) -> None:
    unlink(path)


def rmdir(path: Any) -> None:
    _Path.rmdir(path)


def mkdir(path: Any, mode: int = 0o777) -> None:
    _require_cap("fs.write")
    intrinsic = _require_intrinsic("_molt_path_mkdir", _MOLT_PATH_MKDIR)
    intrinsic(path)


def chmod(path: Any, mode: int) -> None:
    _require_cap("fs.write")
    intrinsic = _require_intrinsic("_molt_path_chmod", _MOLT_PATH_CHMOD)
    intrinsic(path, mode)


def makedirs(name: Any, mode: int = 0o777, exist_ok: bool = False) -> None:
    path = fspath(name)
    if not path:
        return
    if isinstance(path, bytes):
        path = path.decode("utf-8", "surrogateescape")
    parts: list[str] = []
    for part in path.split(sep):
        if not part:
            if not parts:
                parts.append(sep)
            continue
        parts.append(part)
        current = parts[0]
        if len(parts) > 1:
            for extra in parts[1:]:
                current = _Path.join(current, extra)
        if _Path.exists(current):
            continue
        try:
            mkdir(current, mode)
        except FileExistsError:
            if not exist_ok:
                raise
    if not exist_ok and not _Path.exists(path):
        raise FileNotFoundError(path)


def close(fd: int) -> None:
    intrinsic = _require_intrinsic("_molt_os_close", _MOLT_OS_CLOSE)
    intrinsic(fd)


def dup(fd: int) -> int:
    intrinsic = _require_intrinsic("_molt_os_dup", _MOLT_OS_DUP)
    return int(intrinsic(fd))


def get_inheritable(fd: int) -> bool:
    intrinsic = _require_intrinsic("_molt_os_get_inheritable", _MOLT_OS_GET_INHERITABLE)
    return bool(intrinsic(fd))


def set_inheritable(fd: int, inheritable: bool) -> None:
    intrinsic = _require_intrinsic("_molt_os_set_inheritable", _MOLT_OS_SET_INHERITABLE)
    intrinsic(fd, bool(inheritable))
