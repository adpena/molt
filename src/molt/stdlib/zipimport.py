"""Intrinsic-backed zipimport support for Molt (CPython 3.12+ surface)."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

from importlib.machinery import ModuleSpec as _ModuleSpec
from importlib.machinery import ZipSourceLoader as _ZipSourceLoader
from types import ModuleType as _ModuleType
from typing import Any as _Any
import marshal as _marshal
import os as _os
import sys as _sys
import time as _time

__all__ = ["zipimporter", "ZipImportError"]


class ZipImportError(ImportError):
    pass


END_CENTRAL_DIR_SIZE = 22
MAX_COMMENT_LEN = (1 << 16) - 1
STRING_END_ARCHIVE = b"PK\x05\x06"
path_sep = _os.sep
alt_path_sep = _os.altsep or ""
cp437_table = bytes(range(256)).decode("cp437")
marshal = _marshal
sys = _sys
time = _time

_MOLT_IMPORTLIB_FIND_IN_PATH_PACKAGE_CONTEXT = _require_intrinsic(
    "molt_importlib_find_in_path_package_context", globals()
)
_MOLT_IMPORTLIB_ZIP_SOURCE_EXEC_PAYLOAD = _require_intrinsic(
    "molt_importlib_zip_source_exec_payload", globals()
)
_MOLT_IMPORTLIB_ZIP_READ_ENTRY = _require_intrinsic(
    "molt_importlib_zip_read_entry", globals()
)
_MOLT_CAPABILITIES_TRUSTED = _require_intrinsic("molt_capabilities_trusted", globals())
_MOLT_CAPABILITIES_REQUIRE = _require_intrinsic("molt_capabilities_require", globals())


def _split_archive_path(path: str) -> tuple[str, str]:
    text = str(path)
    if not text:
        return "", ""
    lower = text.lower()
    idx = lower.rfind(".zip")
    if idx == -1:
        return "", text
    idx += 4
    archive = text[:idx]
    rest = text[idx:]
    if rest.startswith(("/", _os.sep)):
        rest = rest.lstrip("/\\")
    rest = rest.replace("\\", "/").strip("/")
    return archive, rest


def _normalize_inner_path(path: str) -> str:
    return str(path).replace("\\", "/").strip("/")


def _validate_resolution(payload: _Any) -> dict[str, _Any]:
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


def _source_payload_from_resolution(
    fullname: str, payload: dict[str, _Any]
) -> dict[str, _Any]:
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
    return source_payload


class zipimporter:
    def __init__(self, path: str) -> None:
        if not _MOLT_CAPABILITIES_TRUSTED():
            _MOLT_CAPABILITIES_REQUIRE("fs.read")
        archive_path = str(path)
        archive, prefix = _split_archive_path(archive_path)
        self.archive = archive
        self.prefix = f"{prefix}/" if prefix else ""
        if not self.archive:
            raise ZipImportError("archive path is empty")
        self._search_root = self.archive if not prefix else f"{self.archive}/{prefix}"

    def _resolve(self, fullname: str) -> dict[str, _Any]:
        payload = _MOLT_IMPORTLIB_FIND_IN_PATH_PACKAGE_CONTEXT(
            fullname, [self._search_root]
        )
        if payload is None:
            raise ZipImportError(f"can't find module {fullname!r}")
        return _validate_resolution(payload)

    def _loader_from_payload(self, fullname: str, payload: dict[str, _Any]):
        return _ZipSourceLoader(
            fullname, payload["zip_archive"], payload["zip_inner_path"]
        )

    def _spec_from_payload(
        self, fullname: str, payload: dict[str, _Any]
    ) -> _ModuleSpec:
        loader = self._loader_from_payload(fullname, payload)
        origin = payload.get("origin")
        if not isinstance(origin, str):
            origin = f"{payload['zip_archive']}/{payload['zip_inner_path']}"
        is_package = bool(payload.get("is_package"))
        spec = _ModuleSpec(
            fullname, loader=loader, origin=origin, is_package=is_package
        )
        if is_package:
            locations = payload.get("submodule_search_locations")
            if isinstance(locations, list) and all(
                isinstance(entry, str) for entry in locations
            ):
                spec.submodule_search_locations = list(locations)
            elif spec.submodule_search_locations is None:
                spec.submodule_search_locations = [
                    f"{payload['zip_archive']}/{fullname.replace('.', '/')}"
                ]
        return spec

    def find_spec(self, fullname: str, target: object | None = None):
        _ = target
        try:
            payload = self._resolve(fullname)
        except ZipImportError:
            return None
        return self._spec_from_payload(fullname, payload)

    def create_module(self, spec: _ModuleSpec):
        _ = spec
        return None

    def exec_module(self, module: _ModuleType) -> None:
        module_name = getattr(module, "__name__", None)
        if not isinstance(module_name, str):
            raise ZipImportError("module name must be str")
        payload = self._resolve(module_name)
        loader = self._loader_from_payload(module_name, payload)
        loader.exec_module(module)

    def load_module(self, fullname: str):
        existing = _sys.modules.get(fullname)
        if existing is not None:
            return existing
        payload = self._resolve(fullname)
        loader = self._loader_from_payload(fullname, payload)
        spec = self._spec_from_payload(fullname, payload)
        module = _ModuleType(fullname)
        module.__spec__ = spec
        module.__loader__ = loader
        module.__file__ = spec.origin
        module.__cached__ = None
        if spec.submodule_search_locations is not None:
            module.__package__ = fullname
            module.__path__ = list(spec.submodule_search_locations)
        else:
            module.__package__ = fullname.rpartition(".")[0]
        _sys.modules[fullname] = module
        try:
            loader.exec_module(module)
        except BaseException:
            _sys.modules.pop(fullname, None)
            raise
        return module

    def get_data(self, pathname: str) -> bytes:
        raw = str(pathname)
        archive, inner = _split_archive_path(raw)
        if archive:
            if archive != self.archive:
                raise OSError(f"zipimporter cannot handle path {raw!r}")
        else:
            inner = _normalize_inner_path(raw)
            archive = self.archive
        inner = _normalize_inner_path(inner)
        if not inner:
            raise OSError(f"can't read archive root {raw!r}")
        data = _MOLT_IMPORTLIB_ZIP_READ_ENTRY(archive, inner)
        if not isinstance(data, bytes):
            raise ZipImportError("invalid zip entry payload: bytes expected")
        return data

    def get_code(self, fullname: str):
        source = self.get_source(fullname)
        if source is None:
            return None
        return compile(source, self.get_filename(fullname), "exec", dont_inherit=True)

    def get_source(self, fullname: str) -> str | None:
        payload = self._resolve(fullname)
        source_payload = _source_payload_from_resolution(fullname, payload)
        return source_payload["source"]

    def get_filename(self, fullname: str) -> str:
        payload = self._resolve(fullname)
        origin = payload.get("origin")
        if isinstance(origin, str):
            return origin
        return f"{payload['zip_archive']}/{payload['zip_inner_path']}"

    def is_package(self, fullname: str) -> bool:
        payload = self._resolve(fullname)
        return bool(payload.get("is_package"))

    def invalidate_caches(self) -> None:
        return None

    def get_resource_reader(self, fullname: str):
        try:
            payload = self._resolve(fullname)
        except ZipImportError:
            return None
        loader = self._loader_from_payload(fullname, payload)
        get_reader = getattr(loader, "get_resource_reader", None)
        if not callable(get_reader):
            return None
        return get_reader(fullname)
