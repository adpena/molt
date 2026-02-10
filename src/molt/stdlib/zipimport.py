"""Minimal intrinsic-backed zipimport support for Molt."""

# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): Expand zipimport API surface beyond load_module/find_module/get_source.

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

from importlib.machinery import ModuleSpec, ZipSourceLoader
from types import ModuleType
from typing import Any
import os
import sys

__all__ = ["zipimporter", "ZipImportError"]


class ZipImportError(ImportError):
    pass


_MOLT_IMPORTLIB_FIND_IN_PATH_PACKAGE_CONTEXT = _require_intrinsic(
    "molt_importlib_find_in_path_package_context", globals()
)
_MOLT_IMPORTLIB_ZIP_SOURCE_EXEC_PAYLOAD = _require_intrinsic(
    "molt_importlib_zip_source_exec_payload", globals()
)

_capabilities: ModuleType | None
try:
    from molt import capabilities as _capabilities_raw
except Exception:
    _capabilities = None
else:
    _capabilities = (
        _capabilities_raw if isinstance(_capabilities_raw, ModuleType) else None
    )


def _split_archive_path(path: str) -> tuple[str, str]:
    if not path:
        return "", ""
    lower = path.lower()
    idx = lower.rfind(".zip")
    if idx == -1:
        return path, ""
    idx += 4
    archive = path[:idx]
    rest = path[idx:]
    if rest.startswith(("/", os.sep)):
        rest = rest.lstrip("/\\")
    rest = rest.replace("\\", "/").strip("/")
    return archive, rest


def _validate_resolution(payload: Any) -> dict[str, Any]:
    if not isinstance(payload, dict):
        raise ZipImportError("invalid zipimport resolution payload")
    loader_kind = payload.get("loader_kind")
    zip_archive = payload.get("zip_archive")
    zip_inner_path = payload.get("zip_inner_path")
    if loader_kind != "zip_source":
        raise ZipImportError("only zip source modules are supported")
    if not isinstance(zip_archive, str) or not zip_archive:
        raise ZipImportError("missing zip archive path")
    if not isinstance(zip_inner_path, str) or not zip_inner_path:
        raise ZipImportError("missing zip inner path")
    return payload


class zipimporter:
    def __init__(self, archive: str) -> None:
        if _capabilities is not None and not _capabilities.trusted():
            _capabilities.require("fs.read")
        archive_path = str(archive)
        self.archive, self._prefix = _split_archive_path(archive_path)
        if not self.archive:
            raise ZipImportError("archive path is empty")
        self._search_root = (
            self.archive if not self._prefix else f"{self.archive}/{self._prefix}"
        )

    def _resolve(self, fullname: str) -> dict[str, Any]:
        payload = _MOLT_IMPORTLIB_FIND_IN_PATH_PACKAGE_CONTEXT(
            fullname, [self._search_root]
        )
        if payload is None:
            raise ZipImportError(f"can't find module {fullname!r}")
        return _validate_resolution(payload)

    def load_module(self, fullname: str):
        existing = sys.modules.get(fullname)
        if existing is not None:
            return existing
        payload = self._resolve(fullname)
        zip_archive = payload["zip_archive"]
        zip_inner_path = payload["zip_inner_path"]
        loader = ZipSourceLoader(fullname, zip_archive, zip_inner_path)
        origin = payload.get("origin")
        if not isinstance(origin, str):
            origin = f"{zip_archive}/{zip_inner_path}"
        is_package = bool(payload.get("is_package"))
        module = ModuleType(fullname)
        spec = ModuleSpec(fullname, loader=loader, origin=origin, is_package=is_package)
        module.__spec__ = spec
        module.__loader__ = loader
        module.__file__ = origin
        module.__cached__ = None
        if is_package:
            module.__package__ = fullname
            locations = payload.get("submodule_search_locations")
            if isinstance(locations, list) and all(
                isinstance(entry, str) for entry in locations
            ):
                module.__path__ = list(locations)
            else:
                module.__path__ = [f"{zip_archive}/{fullname.replace('.', '/')}"]
            if spec.submodule_search_locations is None:
                spec.submodule_search_locations = list(module.__path__)
            else:
                spec.submodule_search_locations[:] = list(module.__path__)
        else:
            module.__package__ = fullname.rpartition(".")[0]
        sys.modules[fullname] = module
        loader.exec_module(module)
        return module

    def find_module(self, fullname: str, path=None):
        try:
            self._resolve(fullname)
        except ZipImportError:
            return None
        return self

    def get_source(self, fullname: str) -> str | None:
        try:
            payload = self._resolve(fullname)
        except ZipImportError:
            return None
        source_payload = _MOLT_IMPORTLIB_ZIP_SOURCE_EXEC_PAYLOAD(
            fullname,
            payload["zip_archive"],
            payload["zip_inner_path"],
            bool(payload.get("is_package")),
        )
        if not isinstance(source_payload, dict):
            raise ZipImportError("invalid zip source payload")
        source = source_payload.get("source")
        if not isinstance(source, str):
            raise ZipImportError("invalid zip source payload: source")
        return source
