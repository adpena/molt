"""Minimal importlib.resources implementation for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

import io as _io
from typing import Iterable as _Iterable
from typing import Union as _Union
import importlib as _importlib
import os as _os
import sys as _sys

from . import abc
from . import readers  # noqa: F401
from .abc import ResourceReader

_require_intrinsic("molt_stdlib_probe", globals())
_MOLT_IMPORTLIB_RESOURCES_PATH_PAYLOAD = _require_intrinsic(
    "molt_importlib_resources_path_payload", globals()
)
_MOLT_IMPORTLIB_RESOURCES_PACKAGE_INFO = _require_intrinsic(
    "molt_importlib_resources_package_info", globals()
)
_MOLT_IMPORTLIB_RESOURCES_OPEN_RESOURCE_BYTES_FROM_PACKAGE = _require_intrinsic(
    "molt_importlib_resources_open_resource_bytes_from_package", globals()
)
_MOLT_IMPORTLIB_RESOURCES_CONTENTS_FROM_PACKAGE = _require_intrinsic(
    "molt_importlib_resources_contents_from_package", globals()
)
_MOLT_IMPORTLIB_RESOURCES_IS_RESOURCE_FROM_PACKAGE = _require_intrinsic(
    "molt_importlib_resources_is_resource_from_package", globals()
)
_MOLT_IMPORTLIB_RESOURCES_RESOURCE_PATH_FROM_PACKAGE = _require_intrinsic(
    "molt_importlib_resources_resource_path_from_package", globals()
)
_MOLT_IMPORTLIB_RESOURCES_READ_TEXT_FROM_PACKAGE = _require_intrinsic(
    "molt_importlib_resources_read_text_from_package", globals()
)
_MOLT_IMPORTLIB_RESOURCES_AS_FILE_ENTER = _require_intrinsic(
    "molt_importlib_resources_as_file_enter", globals()
)
_MOLT_IMPORTLIB_RESOURCES_AS_FILE_EXIT = _require_intrinsic(
    "molt_importlib_resources_as_file_exit", globals()
)
_MOLT_IMPORTLIB_RESOURCES_MODULE_NAME = _require_intrinsic(
    "molt_importlib_resources_module_name", globals()
)
_MOLT_IMPORTLIB_RESOURCES_LOADER_READER = _require_intrinsic(
    "molt_importlib_resources_loader_reader", globals()
)
_MOLT_IMPORTLIB_RESOURCES_FILES_PAYLOAD = _require_intrinsic(
    "molt_importlib_resources_files_payload", globals()
)
_MOLT_IMPORTLIB_RESOURCES_READER_FILES_TRAVERSABLE = _require_intrinsic(
    "molt_importlib_resources_reader_files_traversable", globals()
)
_MOLT_IMPORTLIB_RESOURCES_READER_ROOTS = _require_intrinsic(
    "molt_importlib_resources_reader_roots", globals()
)
_MOLT_IMPORTLIB_RESOURCES_READER_CONTENTS = _require_intrinsic(
    "molt_importlib_resources_reader_contents", globals()
)
_MOLT_IMPORTLIB_RESOURCES_READER_RESOURCE_PATH = _require_intrinsic(
    "molt_importlib_resources_reader_resource_path", globals()
)
_MOLT_IMPORTLIB_RESOURCES_READER_IS_RESOURCE = _require_intrinsic(
    "molt_importlib_resources_reader_is_resource", globals()
)
_MOLT_IMPORTLIB_RESOURCES_READER_OPEN_RESOURCE_BYTES = _require_intrinsic(
    "molt_importlib_resources_reader_open_resource_bytes", globals()
)
_MOLT_IMPORTLIB_RESOURCES_READER_CHILD_NAMES = _require_intrinsic(
    "molt_importlib_resources_reader_child_names", globals()
)
_MOLT_IMPORTLIB_RESOURCES_READER_EXISTS = _require_intrinsic(
    "molt_importlib_resources_reader_exists", globals()
)
_MOLT_IMPORTLIB_RESOURCES_READER_IS_DIR = _require_intrinsic(
    "molt_importlib_resources_reader_is_dir", globals()
)
_MOLT_IMPORTLIB_VALIDATE_RESOURCE_NAME = _require_intrinsic(
    "molt_importlib_validate_resource_name", globals()
)

Package = _Union[str, object]
Anchor = Package
Resource = str

__all__ = [
    "abc",
    "Package",
    "Anchor",
    "Resource",
    "ResourceReader",
    "as_file",
    "contents",
    "files",
    "is_resource",
    "open_binary",
    "open_text",
    "path",
    "read_binary",
    "read_text",
]


class _Traversable:
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
        base = _os.path.basename(self._path)
        if "." not in base or base in {".", ".."}:
            return ""
        return base[base.rfind(".") :]

    def joinpath(self, *parts: str) -> "_Traversable":
        path = self._path
        for part in parts:
            path = _os.path.join(path, part)
        return _Traversable(path)

    def iterdir(self) -> _Iterable["_Traversable"]:
        payload = _resources_path_payload(self._path)
        entries = payload["entries"]
        for entry in entries:
            yield _Traversable(_os.path.join(self._path, entry))
        if "__pycache__" not in entries and payload["has_init_py"]:
            yield _VirtualDirTraversable(_os.path.join(self._path, "__pycache__"))

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
        if not isinstance(mode, str):
            raise TypeError("mode must be str")
        if "b" in mode:
            return open(self._path, mode)
        return open(self._path, mode, encoding=encoding, errors=errors)

    def read_text(self, encoding: str = "utf-8", errors: str = "strict") -> str:
        with self.open("r", encoding=encoding, errors=errors) as handle:
            return handle.read()

    def read_bytes(self) -> bytes:
        with self.open("rb") as handle:
            return handle.read()


class _NamespaceTraversable(_Traversable):
    def __init__(self, roots: list[str], parts: tuple[str, ...] = ()) -> None:
        unique_roots: list[str] = []
        for root in roots:
            if root not in unique_roots:
                unique_roots.append(root)
        if not unique_roots:
            raise RuntimeError("namespace traversable requires at least one root")
        self._roots = unique_roots
        self._parts = parts
        super().__init__(self._candidate_paths()[0])

    def __fspath__(self) -> str:
        return self._candidate_paths()[0]

    def __repr__(self) -> str:
        paths = ", ".join(self._candidate_paths())
        return f"<NamespaceTraversable [{paths}]>"

    @property
    def name(self) -> str:
        if self._parts:
            return self._parts[-1]
        return _resources_path_basename(self._roots[0])

    @property
    def suffix(self) -> str:
        base = self.name
        if "." not in base or base in {".", ".."}:
            return ""
        return base[base.rfind(".") :]

    def joinpath(self, *parts: str) -> "_NamespaceTraversable":
        next_parts = self._parts
        for part in parts:
            if not isinstance(part, str):
                raise TypeError("resource path components must be str")
            next_parts = next_parts + (part,)
        return _NamespaceTraversable(self._roots, next_parts)

    def iterdir(self) -> _Iterable[_Traversable]:
        names: list[str] = []
        has_init_py = False
        for path in self._candidate_paths():
            payload = _resources_path_payload(path)
            if not payload["is_dir"]:
                continue
            entries = payload["entries"]
            for entry in entries:
                if entry not in names:
                    names.append(entry)
            has_init_py = has_init_py or payload["has_init_py"]
        names.sort()
        for name in names:
            yield self.joinpath(name)
        if "__pycache__" not in names and has_init_py:
            yield _VirtualDirTraversable(
                _os.path.join(self.__fspath__(), "__pycache__")
            )

    def is_dir(self) -> bool:
        for path in self._candidate_paths():
            if _resources_path_payload(path)["is_dir"]:
                return True
        return False

    def is_file(self) -> bool:
        for path in self._candidate_paths():
            if _resources_path_payload(path)["is_file"]:
                return True
        return False

    def exists(self) -> bool:
        for path in self._candidate_paths():
            if _resources_path_payload(path)["exists"]:
                return True
        return False

    def open(
        self,
        mode: str = "r",
        encoding: str | None = "utf-8",
        errors: str | None = None,
    ):
        if not isinstance(mode, str):
            raise TypeError("mode must be str")
        if not self.exists():
            raise FileNotFoundError(self.__fspath__())
        if not self.is_file():
            raise IsADirectoryError(self.__fspath__())
        for path in self._candidate_paths():
            payload = _resources_path_payload(path)
            if not payload["is_file"]:
                continue
            if "b" in mode:
                return open(path, mode)
            return open(path, mode, encoding=encoding, errors=errors)
        raise FileNotFoundError(self.__fspath__())

    def _candidate_paths(self) -> list[str]:
        out: list[str] = []
        for root in self._roots:
            path = root
            for part in self._parts:
                path = _os.path.join(path, part)
            out.append(path)
        return out


class _LoaderReaderTraversable(_Traversable):
    def __init__(
        self, reader: object, package_name: str, parts: tuple[str, ...] = ()
    ) -> None:
        self._reader = reader
        self._package_name = package_name
        self._parts = parts
        super().__init__(self._fallback_path())

    def __fspath__(self) -> str:
        name = self._joined_name()
        if not name:
            return self._fallback_path()
        resource_path = _reader_resource_path(self._reader, name)
        if resource_path is not None:
            return resource_path
        return self._fallback_path(name)

    def __repr__(self) -> str:
        return f"<LoaderReaderTraversable {self.__fspath__()}>"

    @property
    def name(self) -> str:
        if self._parts:
            return self._parts[-1]
        return self._package_name.rpartition(".")[2] or self._package_name

    @property
    def suffix(self) -> str:
        base = self.name
        if "." not in base or base in {".", ".."}:
            return ""
        return base[base.rfind(".") :]

    def joinpath(self, *parts: str) -> "_LoaderReaderTraversable":
        next_parts = self._parts
        for part in parts:
            if not isinstance(part, str):
                raise TypeError("resource path components must be str")
            next_parts = next_parts + (part,)
        return _LoaderReaderTraversable(self._reader, self._package_name, next_parts)

    def iterdir(self) -> _Iterable[_Traversable]:
        names = _reader_child_names(self._reader, self._parts)
        for name in sorted(names):
            yield self.joinpath(name)

    def is_dir(self) -> bool:
        if not self._parts:
            return True
        return _reader_is_dir(self._reader, self._parts)

    def is_file(self) -> bool:
        if not self._parts:
            return False
        joined = self._joined_name()
        if not joined:
            return False
        return _reader_is_resource(self._reader, joined)

    def exists(self) -> bool:
        if not self._parts:
            return True
        return _reader_exists(self._reader, self._parts)

    def open(
        self,
        mode: str = "r",
        encoding: str | None = "utf-8",
        errors: str | None = None,
    ):
        if not isinstance(mode, str):
            raise TypeError("mode must be str")
        if mode not in {"r", "rb"}:
            raise ValueError(
                f"Invalid mode value {mode!r}, only 'r' and 'rb' are supported"
            )
        if not self.exists():
            raise FileNotFoundError(self.__fspath__())
        if not self.is_file():
            raise IsADirectoryError(self.__fspath__())
        name = self._joined_name()
        if not name:
            raise FileNotFoundError(self.__fspath__())
        resource_path = _reader_resource_path(self._reader, name)
        if resource_path is not None:
            if "b" in mode:
                return open(resource_path, mode)
            return open(resource_path, mode, encoding=encoding, errors=errors)
        raw = _reader_open_resource_bytes(self._reader, name)
        if "b" in mode:
            return _io.BytesIO(raw)
        return _io.StringIO(raw.decode(encoding or "utf-8", errors=errors or "strict"))

    def _joined_name(self) -> str:
        return "/".join(self._parts)

    def _fallback_path(self, joined: str | None = None) -> str:
        if joined:
            return f"<loader-resource:{self._package_name}/{joined}>"
        return f"<loader-resource:{self._package_name}>"


class _VirtualDirTraversable(_Traversable):
    def iterdir(self) -> _Iterable["_Traversable"]:
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
    value = _MOLT_IMPORTLIB_VALIDATE_RESOURCE_NAME(name)
    if not isinstance(value, str):
        raise RuntimeError("invalid importlib validate resource name payload")
    return value


def _is_write_mode(mode: str) -> bool:
    return any(flag in mode for flag in ("w", "a", "x", "+"))


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
    payload = _MOLT_IMPORTLIB_RESOURCES_PACKAGE_INFO(
        package, tuple(_sys.path), _resources_module_file()
    )
    if not isinstance(payload, tuple) or len(payload) != 4:
        raise RuntimeError(
            "invalid importlib resources package payload: tuple expected"
        )
    roots, is_namespace, has_regular_package, init_file = payload
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


def _resources_files_payload(module: object, fallback: str | None) -> dict[str, object]:
    payload = _MOLT_IMPORTLIB_RESOURCES_FILES_PAYLOAD(
        module, fallback, tuple(_sys.path), _resources_module_file()
    )
    if not isinstance(payload, dict):
        raise RuntimeError("invalid importlib resources files payload: dict expected")
    package_name = payload.get("package_name")
    roots = payload.get("roots")
    is_namespace = payload.get("is_namespace")
    reader = payload.get("reader")
    files_traversable = payload.get("files_traversable")
    if not isinstance(package_name, str) or not package_name:
        raise RuntimeError("invalid importlib resources files payload: package_name")
    if not isinstance(roots, (list, tuple)) or not all(
        isinstance(entry, str) for entry in roots
    ):
        raise RuntimeError("invalid importlib resources files payload: roots")
    if not isinstance(is_namespace, bool):
        raise RuntimeError("invalid importlib resources files payload: is_namespace")
    return {
        "package_name": package_name,
        "roots": list(roots),
        "is_namespace": is_namespace,
        "reader": reader,
        "files_traversable": files_traversable,
    }


def _open_resource_bytes_from_package(package: str, resource: str) -> bytes:
    value = _MOLT_IMPORTLIB_RESOURCES_OPEN_RESOURCE_BYTES_FROM_PACKAGE(
        package, tuple(_sys.path), _resources_module_file(), resource
    )
    if isinstance(value, bytes):
        return value
    if isinstance(value, bytearray):
        return bytes(value)
    raise RuntimeError("invalid importlib resources open payload")


def _resource_contents_from_package(package: str) -> list[str]:
    value = _MOLT_IMPORTLIB_RESOURCES_CONTENTS_FROM_PACKAGE(
        package, tuple(_sys.path), _resources_module_file()
    )
    if not isinstance(value, (list, tuple)) or not all(
        isinstance(entry, str) for entry in value
    ):
        raise RuntimeError("invalid importlib resources contents payload")
    return list(value)


def _resource_is_resource_from_package(package: str, resource: str) -> bool:
    value = _MOLT_IMPORTLIB_RESOURCES_IS_RESOURCE_FROM_PACKAGE(
        package, tuple(_sys.path), _resources_module_file(), resource
    )
    if not isinstance(value, bool):
        raise RuntimeError("invalid importlib resources is_resource payload")
    return value


def _resource_path_from_package(package: str, resource: str) -> str | None:
    value = _MOLT_IMPORTLIB_RESOURCES_RESOURCE_PATH_FROM_PACKAGE(
        package, tuple(_sys.path), _resources_module_file(), resource
    )
    if value is None:
        return None
    if not isinstance(value, str):
        raise RuntimeError("invalid importlib resources path payload")
    return value


def _read_text_from_package(
    package: str, resource: str, encoding: str, errors: str
) -> str:
    value = _MOLT_IMPORTLIB_RESOURCES_READ_TEXT_FROM_PACKAGE(
        package, tuple(_sys.path), _resources_module_file(), resource, encoding, errors
    )
    if not isinstance(value, str):
        raise RuntimeError("invalid importlib resources text payload")
    return value


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
    is_archive_member = payload.get("is_archive_member")
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
    if is_archive_member is None:
        is_archive_member = False
    if not isinstance(is_archive_member, bool):
        raise RuntimeError(
            "invalid importlib resources path payload: is_archive_member"
        )
    return {
        "basename": basename,
        "exists": exists,
        "is_file": is_file,
        "is_dir": is_dir,
        "entries": list(entries),
        "has_init_py": has_init_py,
        "is_archive_member": is_archive_member,
    }


def _resources_path_basename(path: str) -> str:
    value = _resources_path_payload(path)["basename"]
    if not isinstance(value, str):
        raise RuntimeError("invalid importlib resources path payload: basename")
    return value


def _find_namespace_paths(package: str) -> list[str]:
    return _namespace_paths_payload(package)


def _get_package(package: str | object) -> tuple[object, str | None]:
    if isinstance(package, str):
        payload = _resources_package_payload(package)
        roots = payload["roots"]
        is_namespace = bool(payload["is_namespace"])
        has_regular_package = bool(payload["has_regular_package"])
        if has_regular_package:
            return _importlib.import_module(package), package
        if is_namespace and isinstance(roots, list) and roots:
            return _NamespacePackage(package, roots), package
        return _importlib.import_module(package), package
    return package, None


def _package_name(module: object, fallback: str | None) -> str:
    value = _MOLT_IMPORTLIB_RESOURCES_MODULE_NAME(module, fallback)
    if not isinstance(value, str) or not value:
        raise RuntimeError("invalid importlib resources module name payload")
    return value


def _package_root(module: object, fallback: str | None = None) -> str:
    roots, _is_namespace = _package_roots(module, fallback)
    if roots:
        return roots[0]
    module_name = _package_name(module, fallback)
    raise ModuleNotFoundError(module_name)


def _loader_resource_roots(module: object, module_name: str) -> list[str]:
    reader = _loader_resource_reader(module, module_name)
    if reader is None:
        return []
    return _reader_roots(reader)


def _loader_resource_reader(module: object, module_name: str) -> object | None:
    return _MOLT_IMPORTLIB_RESOURCES_LOADER_READER(module, module_name)


def _reader_roots(reader: object) -> list[str]:
    values = _MOLT_IMPORTLIB_RESOURCES_READER_ROOTS(reader)
    if not isinstance(values, (list, tuple)) or not all(
        isinstance(entry, str) for entry in values
    ):
        raise RuntimeError("invalid loader resource roots payload: list expected")
    out: list[str] = []
    for entry in values:
        if entry and entry not in out:
            out.append(entry)
    return out


def _reader_files_traversable(reader: object) -> object | None:
    return _MOLT_IMPORTLIB_RESOURCES_READER_FILES_TRAVERSABLE(reader)


def _is_traversable_like(value: object) -> bool:
    required = ("joinpath", "iterdir", "is_file", "is_dir", "open")
    return all(hasattr(value, name) for name in required)


def _reader_contents(reader: object) -> list[str]:
    values = _MOLT_IMPORTLIB_RESOURCES_READER_CONTENTS(reader)
    if not isinstance(values, (list, tuple)):
        raise RuntimeError("invalid loader resource reader contents payload")
    out: list[str] = []
    for entry in values:
        if isinstance(entry, str) and entry and entry not in out:
            out.append(entry)
    return out


def _reader_resource_path(reader: object, name: str) -> str | None:
    value = _MOLT_IMPORTLIB_RESOURCES_READER_RESOURCE_PATH(reader, name)
    if value is None:
        return None
    if not isinstance(value, str):
        raise RuntimeError("invalid loader resource path payload")
    return value


def _reader_is_resource(reader: object, name: str) -> bool:
    value = _MOLT_IMPORTLIB_RESOURCES_READER_IS_RESOURCE(reader, name)
    if not isinstance(value, bool):
        raise RuntimeError("invalid loader resource is_resource payload")
    return value


def _reader_open_resource_bytes(reader: object, name: str) -> bytes:
    value = _MOLT_IMPORTLIB_RESOURCES_READER_OPEN_RESOURCE_BYTES(reader, name)
    if isinstance(value, bytes):
        return value
    if isinstance(value, bytearray):
        return bytes(value)
    raise RuntimeError("invalid loader open_resource payload")


def _reader_child_names(reader: object, parts: tuple[str, ...]) -> list[str]:
    values = _MOLT_IMPORTLIB_RESOURCES_READER_CHILD_NAMES(reader, parts)
    if not isinstance(values, (list, tuple)):
        raise RuntimeError("invalid loader resource reader contents payload")
    out: list[str] = []
    for entry in values:
        if isinstance(entry, str) and entry and entry not in out:
            out.append(entry)
    return out


def _reader_exists(reader: object, parts: tuple[str, ...]) -> bool:
    value = _MOLT_IMPORTLIB_RESOURCES_READER_EXISTS(reader, parts)
    if not isinstance(value, bool):
        raise RuntimeError("invalid loader resource exists payload")
    return value


def _reader_is_dir(reader: object, parts: tuple[str, ...]) -> bool:
    value = _MOLT_IMPORTLIB_RESOURCES_READER_IS_DIR(reader, parts)
    if not isinstance(value, bool):
        raise RuntimeError("invalid loader resource is_dir payload")
    return value


def _package_roots(
    module: object, fallback: str | None = None
) -> tuple[list[str], bool]:
    payload = _resources_files_payload(module, fallback)
    roots = payload["roots"]
    if not isinstance(roots, list):
        raise RuntimeError("invalid importlib resources files payload: roots")
    is_namespace = payload["is_namespace"]
    if not isinstance(is_namespace, bool):
        raise RuntimeError("invalid importlib resources files payload: is_namespace")
    return roots, is_namespace


def files(package: str | object) -> _Traversable | _NamespaceTraversable:
    module, module_name = _get_package(package)
    payload = _resources_files_payload(module, module_name)
    package_name = payload["package_name"]
    if not isinstance(package_name, str):
        raise RuntimeError("invalid importlib resources files payload: package_name")
    roots = payload["roots"]
    if not isinstance(roots, list):
        raise RuntimeError("invalid importlib resources files payload: roots")
    is_namespace = payload["is_namespace"]
    if not isinstance(is_namespace, bool):
        raise RuntimeError("invalid importlib resources files payload: is_namespace")
    reader = payload["reader"]
    files_traversable = payload["files_traversable"]
    if not roots and reader is not None:
        if files_traversable is not None and _is_traversable_like(files_traversable):
            return files_traversable
        return _LoaderReaderTraversable(reader, package_name)
    if not roots:
        raise ModuleNotFoundError(package_name)
    if is_namespace and len(roots) > 1:
        return _NamespaceTraversable(roots)
    return _Traversable(roots[0])


class _AsFileContext:
    def __init__(self, traversable: _Traversable | object) -> None:
        self._traversable = traversable

    def __enter__(self) -> _Traversable:
        return _MOLT_IMPORTLIB_RESOURCES_AS_FILE_ENTER(self._traversable, _Traversable)

    def __exit__(self, exc_type, exc, tb) -> bool:
        return bool(_MOLT_IMPORTLIB_RESOURCES_AS_FILE_EXIT(exc_type, exc, tb))


def as_file(traversable: _Traversable | object) -> _AsFileContext:
    return _AsFileContext(traversable)


def contents(package: str | object) -> list[str]:
    if isinstance(package, str):
        return _resource_contents_from_package(package)
    root = files(package)
    return sorted([entry.name for entry in root.iterdir()])


def is_resource(package: str | object, name: str) -> bool:
    if isinstance(package, str):
        _validate_resource_name(name)
        return _resource_is_resource_from_package(package, name)
    root = files(package)
    _validate_resource_name(name)
    return root.joinpath(name).is_file()


def open_text(
    package: str | object,
    resource: str,
    encoding: str = "utf-8",
    errors: str = "strict",
):
    if isinstance(package, str):
        _validate_resource_name(resource)
        return _io.StringIO(
            _read_text_from_package(package, resource, encoding=encoding, errors=errors)
        )
    root = files(package)
    _validate_resource_name(resource)
    path = root.joinpath(resource)
    if not path.exists() or not path.is_file():
        raise FileNotFoundError(resource)
    return path.open("r", encoding=encoding, errors=errors)


def open_binary(package: str | object, resource: str):
    if isinstance(package, str):
        _validate_resource_name(resource)
        return _io.BytesIO(_open_resource_bytes_from_package(package, resource))
    root = files(package)
    _validate_resource_name(resource)
    path = root.joinpath(resource)
    if not path.exists() or not path.is_file():
        raise FileNotFoundError(resource)
    return path.open("rb")


def read_text(
    package: str | object,
    resource: str,
    encoding: str = "utf-8",
    errors: str = "strict",
) -> str:
    if isinstance(package, str):
        _validate_resource_name(resource)
        return _read_text_from_package(
            package, resource, encoding=encoding, errors=errors
        )
    root = files(package)
    _validate_resource_name(resource)
    path = root.joinpath(resource)
    if not path.exists() or not path.is_file():
        raise FileNotFoundError(resource)
    return path.read_text(encoding=encoding, errors=errors)


def read_binary(package: str | object, resource: str) -> bytes:
    if isinstance(package, str):
        return _open_resource_bytes_from_package(package, resource)
    root = files(package)
    _validate_resource_name(resource)
    path = root.joinpath(resource)
    if not path.exists() or not path.is_file():
        raise FileNotFoundError(resource)
    return path.read_bytes()


def path(package: str | object, resource: str) -> _AsFileContext:
    _validate_resource_name(resource)
    if isinstance(package, str):
        resource_path = _resource_path_from_package(package, resource)
        if resource_path is not None:
            return as_file(_Traversable(resource_path))
    return as_file(files(package).joinpath(resource))
