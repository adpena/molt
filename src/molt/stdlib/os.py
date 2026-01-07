"""Minimal os shim for Molt."""

from __future__ import annotations

from collections.abc import ItemsView, KeysView, ValuesView
from typing import Any, Iterator, MutableMapping


def _load_py_os() -> Any:
    try:
        import importlib as _importlib

        return _importlib.import_module("os")
    except Exception:
        return None


_py_os = _load_py_os()

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
    return (
        _py_os.environ.get(key, default)
        if _py_os is not None
        else _ENV_STORE.get(key, default)
    )


def _capabilities() -> set[str]:
    raw = str(_molt_env_get("MOLT_CAPABILITIES", ""))
    caps: set[str] = set()
    for cap in raw.split(","):
        stripped = cap.strip()
        if stripped:
            caps.add(stripped)
    return caps


def _require_cap(name: str) -> None:
    try:
        from molt import capabilities

        capabilities.require(name)
        return
    except Exception:
        if name not in _capabilities():
            raise PermissionError("Missing capability")


class _Environ:
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


class _Path:
    sep = sep
    pathsep = pathsep
    curdir = curdir
    pardir = pardir
    extsep = extsep

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


path = _Path()


environ = _Environ()


def getenv(key: str, default: Any = None) -> Any:
    _require_cap("env.read")
    return _molt_env_get(key, default)
