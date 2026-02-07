"""Minimal importlib.resources implementation for Molt."""

# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): add zip import and loader-provided resource reader support.

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

from contextlib import contextmanager
import io
from typing import Iterable
import importlib
import os
import sys

_require_intrinsic("molt_stdlib_probe", globals())
_MOLT_IMPORTLIB_READ_FILE = _require_intrinsic("molt_importlib_read_file", globals())
_MOLT_IMPORTLIB_RESOURCES_PATH_PAYLOAD = _require_intrinsic(
    "molt_importlib_resources_path_payload", globals()
)
_MOLT_IMPORTLIB_RESOURCES_PACKAGE_PAYLOAD = _require_intrinsic(
    "molt_importlib_resources_package_payload", globals()
)

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
        return _resources_path_basename(self._path)

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
        payload = _resources_path_payload(self._path)
        entries = payload["entries"]
        for entry in entries:
            yield Traversable(os.path.join(self._path, entry))
        if "__pycache__" not in entries and payload["has_init_py"]:
            yield _VirtualDirTraversable(os.path.join(self._path, "__pycache__"))

    def is_dir(self) -> bool:
        return _resources_path_payload(self._path)["is_dir"]

    def is_file(self) -> bool:
        return _resources_path_payload(self._path)["is_file"]

    def exists(self) -> bool:
        return _resources_path_payload(self._path)["exists"]

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
        if any(flag in mode for flag in ("w", "a", "x", "+")):
            raise NotImplementedError(
                "importlib.resources Traversable.open write modes"
            )
        raw = _MOLT_IMPORTLIB_READ_FILE(self._path)
        if not isinstance(raw, bytes):
            raise RuntimeError("invalid importlib read payload: bytes expected")
        if "b" in mode:
            return io.BytesIO(raw)
        return io.StringIO(raw.decode(encoding or "utf-8", errors=errors or "strict"))

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


def _resources_module_file() -> str | None:
    module_file = globals().get("__file__")
    if isinstance(module_file, str) and module_file:
        return module_file
    return None


def _resources_package_payload(package: str) -> dict[str, object]:
    payload = _MOLT_IMPORTLIB_RESOURCES_PACKAGE_PAYLOAD(
        package, tuple(sys.path), _resources_module_file()
    )
    if not isinstance(payload, dict):
        raise RuntimeError("invalid importlib resources package payload: dict expected")
    roots = payload.get("roots")
    is_namespace = payload.get("is_namespace")
    has_regular_package = payload.get("has_regular_package")
    init_file = payload.get("init_file")
    if not isinstance(roots, (list, tuple)) or not all(
        isinstance(entry, str) for entry in roots
    ):
        raise RuntimeError("invalid importlib resources package payload: roots")
    if not isinstance(is_namespace, bool):
        raise RuntimeError("invalid importlib resources package payload: is_namespace")
    if not isinstance(has_regular_package, bool):
        raise RuntimeError(
            "invalid importlib resources package payload: has_regular_package"
        )
    if init_file is not None and not isinstance(init_file, str):
        raise RuntimeError("invalid importlib resources package payload: init_file")
    return {
        "roots": list(roots),
        "is_namespace": is_namespace,
        "has_regular_package": has_regular_package,
        "init_file": init_file,
    }


def _namespace_paths_payload(package: str) -> list[str]:
    payload = _resources_package_payload(package)
    roots = payload["roots"]
    if not isinstance(roots, list):
        raise RuntimeError("invalid importlib resources package payload: roots")
    is_namespace = payload["is_namespace"]
    if not isinstance(is_namespace, bool):
        raise RuntimeError("invalid importlib resources package payload: is_namespace")
    if not is_namespace:
        return []
    out: list[str] = []
    for entry in roots:
        if not isinstance(entry, str):
            raise RuntimeError("invalid importlib namespace paths payload: str entries")
        out.append(entry)
    return out


def _resources_path_payload(path: str) -> dict[str, object]:
    payload = _MOLT_IMPORTLIB_RESOURCES_PATH_PAYLOAD(path)
    if not isinstance(payload, dict):
        raise RuntimeError("invalid importlib resources path payload: dict expected")
    basename = payload.get("basename")
    exists = payload.get("exists")
    is_file = payload.get("is_file")
    is_dir = payload.get("is_dir")
    entries = payload.get("entries")
    has_init_py = payload.get("has_init_py")
    if not isinstance(basename, str):
        raise RuntimeError("invalid importlib resources path payload: basename")
    if not isinstance(exists, bool):
        raise RuntimeError("invalid importlib resources path payload: exists")
    if not isinstance(is_file, bool):
        raise RuntimeError("invalid importlib resources path payload: is_file")
    if not isinstance(is_dir, bool):
        raise RuntimeError("invalid importlib resources path payload: is_dir")
    if not isinstance(entries, (list, tuple)) or not all(
        isinstance(entry, str) for entry in entries
    ):
        raise RuntimeError("invalid importlib resources path payload: entries")
    if not isinstance(has_init_py, bool):
        raise RuntimeError("invalid importlib resources path payload: has_init_py")
    return {
        "basename": basename,
        "exists": exists,
        "is_file": is_file,
        "is_dir": is_dir,
        "entries": list(entries),
        "has_init_py": has_init_py,
    }


def _resources_path_basename(path: str) -> str:
    value = _resources_path_payload(path)["basename"]
    if not isinstance(value, str):
        raise RuntimeError("invalid importlib resources path payload: basename")
    return value


def _find_namespace_paths(package: str) -> list[str]:
    return _namespace_paths_payload(package)


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
    module_name = getattr(module, "__name__", None)
    if isinstance(module_name, str) and module_name:
        payload = _resources_package_payload(module_name)
        roots = payload["roots"]
        if isinstance(roots, list) and roots:
            return roots[0]
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
    _validate_resource_name(resource)
    path = files(package).joinpath(resource)
    if not path.exists() or not path.is_file():
        raise FileNotFoundError(resource)
    raw = _MOLT_IMPORTLIB_READ_FILE(path.__fspath__())
    if not isinstance(raw, bytes):
        raise RuntimeError("invalid importlib read payload: bytes expected")
    return raw.decode(encoding, errors=errors)


def read_binary(package: str | object, resource: str) -> bytes:
    _validate_resource_name(resource)
    path = files(package).joinpath(resource)
    if not path.exists() or not path.is_file():
        raise FileNotFoundError(resource)
    raw = _MOLT_IMPORTLIB_READ_FILE(path.__fspath__())
    if not isinstance(raw, bytes):
        raise RuntimeError("invalid importlib read payload: bytes expected")
    return raw
