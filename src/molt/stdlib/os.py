"""Minimal os shim for Molt."""

from __future__ import annotations

from collections.abc import ItemsView, KeysView, ValuesView
from typing import Any, Iterator, MutableMapping

try:
    import importlib as _importlib

    _py_os = _importlib.import_module("os")
except Exception:
    _py_os = None

__all__ = [
    "name",
    "sep",
    "pathsep",
    "linesep",
    "curdir",
    "pardir",
    "extsep",
    "altsep",
    "getenv",
    "environ",
    "path",
]

name = getattr(_py_os, "name", "posix") if _py_os is not None else "posix"
sep = "/"
pathsep = ":"
linesep = "\n"
curdir = "."
pardir = ".."
extsep = "."
altsep = None


_ENV_STORE: dict[str, str] = {}


def _molt_env_get(key: str, default: Any = None) -> Any:
    if _py_os is not None:
        return _py_os.environ.get(key, default)
    return _ENV_STORE.get(key, default)


def _capabilities() -> set[str]:
    raw = str(_molt_env_get("MOLT_CAPABILITIES", ""))
    return {cap.strip() for cap in raw.split(",") if cap.strip()}


def _require_cap(name: str) -> None:
    try:
        from molt import capabilities

        capabilities.require(name)
        return
    except Exception:
        if name not in _capabilities():
            raise PermissionError("Missing capability")


class _Environ(MutableMapping[str, str]):
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
        del self._backend()[key]

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


class _Path:
    sep = sep
    pathsep = pathsep
    curdir = curdir
    pardir = pardir
    extsep = extsep

    @staticmethod
    def join(*parts: str) -> str:
        if not parts:
            return ""
        cleaned = [p for p in parts if p]
        if not cleaned:
            return ""
        path = cleaned[0]
        for part in cleaned[1:]:
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
        prefix = sep if absolute else ""
        normalized = prefix + sep.join(parts)
        return normalized or (sep if absolute else ".")

    @staticmethod
    def abspath(path: str) -> str:
        return _Path.normpath(path)


path = _Path()


environ = _Environ()


def getenv(key: str, default: Any = None) -> Any:
    _require_cap("env.read")
    return _molt_env_get(key, default)
