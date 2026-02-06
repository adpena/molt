"""Minimal importlib.resources implementation for Molt."""

# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): add namespace package, zip import, and loader-provided resource reader support.

from __future__ import annotations

from contextlib import contextmanager
from typing import Iterable
import importlib
import os
import sys

from molt import capabilities

__all__ = [
    "files",
    "as_file",
    "read_text",
    "read_binary",
    "open_text",
    "open_binary",
    "contents",
    "is_resource",
]


class Traversable:
    def __init__(self, path: str) -> None:
        self._path = path

    def __fspath__(self) -> str:
        return self._path

    def __repr__(self) -> str:
        return f"<Traversable {self._path}>"

    @property
    def name(self) -> str:
        return os.path.basename(self._path)

    @property
    def suffix(self) -> str:
        base = os.path.basename(self._path)
        if "." not in base or base in {".", ".."}:
            return ""
        return base[base.rfind(".") :]

    def joinpath(self, *parts: str) -> "Traversable":
        path = self._path
        for part in parts:
            path = os.path.join(path, part)
        return Traversable(path)

    def iterdir(self) -> Iterable["Traversable"]:
        entries = os.listdir(self._path)
        for entry in entries:
            yield Traversable(os.path.join(self._path, entry))
        if "__pycache__" not in entries and os.path.isfile(
            os.path.join(self._path, "__init__.py")
        ):
            yield _VirtualDirTraversable(os.path.join(self._path, "__pycache__"))

    def is_dir(self) -> bool:
        return os.path.isdir(self._path)

    def is_file(self) -> bool:
        return os.path.isfile(self._path)

    def exists(self) -> bool:
        return os.path.exists(self._path)

    def open(
        self,
        mode: str = "r",
        encoding: str | None = "utf-8",
        errors: str | None = None,
    ):
        if not self.exists():
            raise FileNotFoundError(self._path)
        if not self.is_file():
            raise IsADirectoryError(self._path)
        if not capabilities.trusted():
            if "r" in mode or "+" in mode:
                capabilities.require("fs.read")
            if "w" in mode or "a" in mode or "+" in mode:
                capabilities.require("fs.write")
        if "b" in mode:
            return open(self._path, mode)
        return open(self._path, mode, encoding=encoding, errors=errors)

    def read_text(self, encoding: str = "utf-8", errors: str = "strict") -> str:
        with self.open("r", encoding=encoding, errors=errors) as handle:
            return handle.read()

    def read_bytes(self) -> bytes:
        with self.open("rb") as handle:
            return handle.read()


class _VirtualDirTraversable(Traversable):
    def iterdir(self) -> Iterable["Traversable"]:
        return iter(())

    def is_dir(self) -> bool:
        return True

    def is_file(self) -> bool:
        return False

    def exists(self) -> bool:
        return True

    def open(
        self,
        mode: str = "r",
        encoding: str | None = "utf-8",
        errors: str | None = None,
    ):
        raise IsADirectoryError(self._path)


def _validate_resource_name(name: str) -> str:
    if not isinstance(name, str):
        raise TypeError("resource name must be str")
    if not name or name in {".", ".."}:
        raise ValueError(f"{name!r} must be only a file name")
    separators = {"/", "\\"}
    if os.sep:
        separators.add(os.sep)
    if os.altsep:
        separators.add(os.altsep)
    for sep in separators:
        if sep and sep in name:
            raise ValueError(f"{name!r} must be only a file name")
    return name


class _NamespacePackage:
    def __init__(self, name: str, paths: list[str]) -> None:
        self.__name__ = name
        self.__path__ = paths


def _find_namespace_paths(package: str) -> list[str]:
    parts = package.split(".")
    search_paths = list(sys.path)
    if "" not in search_paths:
        search_paths.append("")
    try:
        cwd = os.getcwd()
    except Exception:
        cwd = ""
    if cwd and cwd not in search_paths:
        search_paths.append(cwd)
    matches: list[str] = []
    for base in search_paths:
        if not base:
            base = "."
        path = base
        for part in parts:
            path = os.path.join(path, part)
        if os.path.isdir(path):
            matches.append(path)
    return matches


def _get_package(package: str | object):
    if isinstance(package, str):
        try:
            return importlib.import_module(package)
        except ImportError as exc:
            ns_paths = _find_namespace_paths(package)
            if ns_paths:
                return _NamespacePackage(package, ns_paths)
            if isinstance(exc, ModuleNotFoundError):
                raise
            raise ModuleNotFoundError(package) from exc
    return package


def _package_root(module) -> str:
    spec = getattr(module, "__spec__", None)
    if spec is not None:
        search = getattr(spec, "submodule_search_locations", None)
        if search:
            return search[0]
    path_list = getattr(module, "__path__", None)
    if path_list:
        return path_list[0]
    file_attr = getattr(module, "__file__", None)
    if isinstance(file_attr, str):
        if os.path.basename(file_attr) == "__init__.py":
            return os.path.dirname(file_attr)
        return os.path.dirname(file_attr)
    raise ModuleNotFoundError(getattr(module, "__name__", "unknown"))


def files(package: str | object) -> Traversable:
    module = _get_package(package)
    root = _package_root(module)
    return Traversable(root)


@contextmanager
def as_file(traversable: Traversable | object):
    if isinstance(traversable, Traversable):
        yield traversable
        return
    if not isinstance(traversable, (str, bytes, os.PathLike)):
        raise TypeError("as_file expects a Traversable or path-like object")
    path = os.fspath(traversable)
    yield Traversable(os.fsdecode(path))


def contents(package: str | object) -> list[str]:
    root = files(package)
    return sorted([entry.name for entry in root.iterdir()])


def is_resource(package: str | object, name: str) -> bool:
    _validate_resource_name(name)
    root = files(package)
    return root.joinpath(name).is_file()


def open_text(
    package: str | object,
    resource: str,
    encoding: str = "utf-8",
    errors: str = "strict",
):
    _validate_resource_name(resource)
    path = files(package).joinpath(resource)
    if not path.exists() or not path.is_file():
        raise FileNotFoundError(resource)
    return path.open("r", encoding=encoding, errors=errors)


def open_binary(package: str | object, resource: str):
    _validate_resource_name(resource)
    path = files(package).joinpath(resource)
    if not path.exists() or not path.is_file():
        raise FileNotFoundError(resource)
    return path.open("rb")


def read_text(
    package: str | object,
    resource: str,
    encoding: str = "utf-8",
    errors: str = "strict",
) -> str:
    handle = open_text(package, resource, encoding=encoding, errors=errors)
    try:
        return handle.read()
    finally:
        try:
            handle.close()
        except Exception:
            pass


def read_binary(package: str | object, resource: str) -> bytes:
    handle = open_binary(package, resource)
    try:
        return handle.read()
    finally:
        try:
            handle.close()
        except Exception:
            pass
