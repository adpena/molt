"""Minimal os shim for Molt."""

from __future__ import annotations

from collections.abc import ItemsView, KeysView, ValuesView
from typing import TYPE_CHECKING, Any, Iterator, MutableMapping

import builtins as _builtins


def _load_intrinsic(name: str) -> Any | None:
    direct = globals().get(name)
    if direct is not None:
        return direct
    return getattr(_builtins, name, None)


_MOLT_ENV_GET_RAW = _load_intrinsic("_molt_env_get_raw")
_MOLT_OS_NAME = _load_intrinsic("_molt_os_name")
_MOLT_PATH_EXISTS = _load_intrinsic("_molt_path_exists")
_MOLT_PATH_LISTDIR = _load_intrinsic("_molt_path_listdir")
_MOLT_PATH_MKDIR = _load_intrinsic("_molt_path_mkdir")
_MOLT_PATH_UNLINK = _load_intrinsic("_molt_path_unlink")
_MOLT_PATH_RMDIR = _load_intrinsic("_molt_path_rmdir")
_MOLT_PATH_CHMOD = _load_intrinsic("_molt_path_chmod")
_MOLT_GETCWD = _load_intrinsic("_molt_getcwd")
_MOLT_OS_CLOSE = _load_intrinsic("_molt_os_close")
_MOLT_OS_DUP = _load_intrinsic("_molt_os_dup")
_MOLT_OS_GET_INHERITABLE = _load_intrinsic("_molt_os_get_inheritable")
_MOLT_OS_SET_INHERITABLE = _load_intrinsic("_molt_os_set_inheritable")


def _should_load_py_os() -> bool:
    return (
        _MOLT_ENV_GET_RAW is None
        and _MOLT_OS_NAME is None
        and _MOLT_PATH_EXISTS is None
        and _MOLT_PATH_LISTDIR is None
        and _MOLT_PATH_MKDIR is None
        and _MOLT_PATH_UNLINK is None
        and _MOLT_PATH_RMDIR is None
        and _MOLT_PATH_CHMOD is None
        and _MOLT_GETCWD is None
    )


def _load_py_os() -> Any:
    if not _should_load_py_os():
        return None
    try:
        import importlib as _importlib

        module = _importlib.import_module("os")
    except Exception:
        return None
    if module is None:
        return None
    if getattr(module, "__name__", None) == __name__:
        return None
    return module


_py_os = _load_py_os()


def _resolve_os_name() -> str:
    if callable(_MOLT_OS_NAME):
        try:
            value = _MOLT_OS_NAME()
            if isinstance(value, str):
                return value
        except Exception:
            pass
    if _py_os is not None:
        try:
            value = getattr(_py_os, "name", None)
            if isinstance(value, str):
                return value
        except Exception:
            pass
    return "posix"


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

if TYPE_CHECKING:

    def _molt_env_get_raw(key: str, default: Any = None) -> Any:
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
    if callable(_MOLT_ENV_GET_RAW):
        try:
            return _MOLT_ENV_GET_RAW(key, default)
        except Exception:
            pass
    if _py_os is not None:
        try:
            env = getattr(_py_os, "environ", None)
            if env is not None and hasattr(env, "get"):
                return env.get(key, default)
        except Exception:
            pass
    if key in _ENV_STORE:
        return _ENV_STORE[key]
    return default


def _capabilities() -> set[str]:
    raw = str(_molt_env_get("MOLT_CAPABILITIES", ""))
    caps: set[str] = set()
    for cap in raw.split(","):
        stripped = cap.strip()
        if stripped:
            caps.add(stripped)
    return caps


def _trusted() -> bool:
    raw = str(_molt_env_get("MOLT_TRUSTED", ""))
    return raw.strip().lower() in {"1", "true", "yes", "on"}


def _require_cap(name: str) -> None:
    try:
        from molt import capabilities

        capabilities.require(name)
        return
    except Exception:
        if _trusted():
            return
        if name not in _capabilities():
            raise PermissionError("Missing capability")


class _Environ:
    # TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): os.environ parity (mapping methods + backend).
    def __init__(self) -> None:
        self._store = _ENV_STORE

    def _backend(self) -> MutableMapping[str, str]:
        if _py_os is not None:
            return _py_os.environ
        return self._store

    def __getitem__(self, key: str) -> str:
        _require_cap("env.read")
        return self._backend()[key]

    def __setitem__(self, key: str, value: str) -> None:
        _require_cap("env.write")
        self._backend()[key] = value

    def __delitem__(self, key: str) -> None:
        _require_cap("env.write")
        self._backend().pop(key)

    def __iter__(self) -> Iterator[str]:
        _require_cap("env.read")
        return iter(self._backend())

    def __len__(self) -> int:
        _require_cap("env.read")
        return len(self._backend())

    def get(self, key: str, default: Any = None) -> Any:
        _require_cap("env.read")
        return self._backend().get(key, default)

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
            if part.startswith(sep):
                path = part
            else:
                if not path.endswith(sep):
                    path += sep
                path += part
        return path

    @staticmethod
    def isabs(path: str) -> bool:
        return path.startswith(sep)

    @staticmethod
    def dirname(path: str) -> str:
        if not path:
            return ""
        stripped = path.rstrip(sep)
        if not stripped:
            return sep
        idx = stripped.rfind(sep)
        if idx == -1:
            return ""
        if idx == 0:
            return sep
        return stripped[:idx]

    @staticmethod
    def basename(path: str) -> str:
        if not path:
            return ""
        stripped = path.rstrip(sep)
        if not stripped:
            return sep
        idx = stripped.rfind(sep)
        if idx == -1:
            return stripped
        return stripped[idx + 1 :]

    @staticmethod
    def split(path: str) -> tuple[str, str]:
        return _Path.dirname(path), _Path.basename(path)

    @staticmethod
    def splitext(path: str) -> tuple[str, str]:
        base = _Path.basename(path)
        if "." not in base or base == "." or base == "..":
            return path, ""
        idx = base.rfind(".")
        root = path[: len(path) - len(base) + idx]
        return root, base[idx:]

    @staticmethod
    def normpath(path: str) -> str:
        if path == "":
            return "."
        absolute = path.startswith(sep)
        parts = []
        for part in path.split(sep):
            if part == "" or part == ".":
                continue
            if part == "..":
                if parts and parts[-1] != "..":
                    parts.pop()
                elif not absolute:
                    parts.append(part)
                continue
            parts.append(part)
        if absolute:
            normalized = sep + sep.join(parts)
            if normalized:
                return normalized
            return sep
        normalized = sep.join(parts)
        if normalized:
            return normalized
        return "."

    @staticmethod
    def abspath(path: str) -> str:
        return _Path.normpath(path)

    @staticmethod
    def exists(path: Any) -> bool:
        _require_cap("fs.read")
        if callable(_MOLT_PATH_EXISTS):
            try:
                res = _MOLT_PATH_EXISTS(path)
                if res is None:
                    return False
                return res
            except Exception:
                pass
        if _py_os is not None:
            try:
                return _py_os.path.exists(path)
            except Exception:
                return False
        return False

    @staticmethod
    def isdir(path: Any) -> bool:
        _require_cap("fs.read")
        if callable(_MOLT_PATH_LISTDIR):
            try:
                _MOLT_PATH_LISTDIR(path)
                return True
            except Exception:
                return False
        if _py_os is not None:
            try:
                return _py_os.path.isdir(path)
            except Exception:
                return False
        return False

    @staticmethod
    def isfile(path: Any) -> bool:
        _require_cap("fs.read")
        if callable(_MOLT_PATH_LISTDIR):
            try:
                _MOLT_PATH_LISTDIR(path)
                return False
            except FileNotFoundError:
                return False
            except Exception:
                return True
        if _py_os is not None:
            try:
                return _py_os.path.isfile(path)
            except Exception:
                return False
        return False

    @staticmethod
    def unlink(path: Any) -> None:
        _require_cap("fs.write")
        if callable(_MOLT_PATH_UNLINK):
            try:
                _MOLT_PATH_UNLINK(path)
                return
            except Exception:
                pass
        if _py_os is not None:
            _py_os.unlink(path)
            return
        raise FileNotFoundError(path)

    @staticmethod
    def rmdir(path: Any) -> None:
        _require_cap("fs.write")
        if callable(_MOLT_PATH_RMDIR):
            try:
                _MOLT_PATH_RMDIR(path)
                return
            except Exception:
                pass
        if _py_os is not None:
            _py_os.rmdir(path)
            return
        raise FileNotFoundError(path)


path = _Path()


def listdir(path: Any = ".") -> list[str]:
    _require_cap("fs.read")
    if callable(_MOLT_PATH_LISTDIR):
        try:
            res = _MOLT_PATH_LISTDIR(path)
            if isinstance(res, list):
                return res
        except Exception:
            raise
    if _py_os is not None:
        return list(_py_os.listdir(path))
    raise FileNotFoundError(path)


environ = _Environ()


def getpid() -> int:
    try:
        return _molt_getpid()  # type: ignore[unresolved-reference]
    except Exception:
        pass
    if _py_os is not None:
        return _py_os.getpid()
    return 0


def getcwd() -> str:
    _require_cap("fs.read")
    if callable(_MOLT_GETCWD):
        return _MOLT_GETCWD()
    if _py_os is not None:
        try:
            return _py_os.getcwd()
        except Exception:
            pass
    for key in ("PWD", "CD", "CWD"):
        value = _molt_env_get(key, None)
        if isinstance(value, str) and value:
            return value
    return curdir


def getenv(key: str, default: Any = None) -> Any:
    _require_cap("env.read")
    return _molt_env_get(key, default)


def unlink(path: Any) -> None:
    _Path.unlink(path)


def rmdir(path: Any) -> None:
    _Path.rmdir(path)


def mkdir(path: Any, mode: int = 0o777) -> None:
    _require_cap("fs.write")
    if callable(_MOLT_PATH_MKDIR):
        _MOLT_PATH_MKDIR(path)
        return
    if _py_os is not None:
        _py_os.mkdir(path, mode)
        return
    raise FileNotFoundError(path)


def chmod(path: Any, mode: int) -> None:
    _require_cap("fs.write")
    if callable(_MOLT_PATH_CHMOD):
        _MOLT_PATH_CHMOD(path, mode)
        return
    if _py_os is not None:
        _py_os.chmod(path, mode)
        return
    raise FileNotFoundError(path)


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
    if callable(_MOLT_OS_CLOSE):
        _MOLT_OS_CLOSE(fd)
        return
    if _py_os is not None:
        _py_os.close(fd)
        return
    raise NotImplementedError("os.close unavailable")


def dup(fd: int) -> int:
    if callable(_MOLT_OS_DUP):
        return int(_MOLT_OS_DUP(fd))
    if _py_os is not None:
        return int(_py_os.dup(fd))
    raise NotImplementedError("os.dup unavailable")


def get_inheritable(fd: int) -> bool:
    if callable(_MOLT_OS_GET_INHERITABLE):
        return bool(_MOLT_OS_GET_INHERITABLE(fd))
    if _py_os is not None and hasattr(_py_os, "get_inheritable"):
        return bool(_py_os.get_inheritable(fd))
    raise NotImplementedError("os.get_inheritable unavailable")


def set_inheritable(fd: int, inheritable: bool) -> None:
    if callable(_MOLT_OS_SET_INHERITABLE):
        _MOLT_OS_SET_INHERITABLE(fd, bool(inheritable))
        return
    if _py_os is not None and hasattr(_py_os, "set_inheritable"):
        _py_os.set_inheritable(fd, bool(inheritable))
        return
    raise NotImplementedError("os.set_inheritable unavailable")
