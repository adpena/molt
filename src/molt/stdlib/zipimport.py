"""Minimal zipimport support for Molt (store-only archives)."""

# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): parity.

from __future__ import annotations

from types import ModuleType
import os
import sys

from importlib.machinery import ModuleSpec
from importlib.machinery import _exec_restricted
from molt import capabilities
from zipfile import BadZipFile, ZipFile

__all__ = ["zipimporter", "ZipImportError"]


class _ZipCacheEntry:
    def __init__(self, archive: str, zf: ZipFile, mtime: float, size: int) -> None:
        self.archive = archive
        self.zf = zf
        self.mtime = mtime
        self.size = size


_ZIP_CACHE: dict[str, _ZipCacheEntry] = {}


class ZipImportError(ImportError):
    pass


class zipimporter:
    def __init__(self, archive: str) -> None:
        if not capabilities.trusted():
            capabilities.require("fs.read")
        archive_path = str(archive)
        self.archive, self._prefix = _split_archive_path(archive_path)
        if not self.archive:
            raise ZipImportError("archive path is empty")

    def load_module(self, fullname: str):
        if fullname in sys.modules:
            return sys.modules[fullname]
        try:
            source, is_package, inner_path = self._get_source(fullname)
        except Exception as exc:
            raise ZipImportError(str(exc)) from exc
        module = ModuleType(fullname)
        origin = f"{self.archive}/{inner_path}"
        spec = ModuleSpec(fullname, loader=self, origin=origin, is_package=is_package)
        module.__spec__ = spec
        module.__loader__ = self
        module.__file__ = origin
        module.__cached__ = None
        if is_package:
            module.__package__ = fullname
            module.__path__ = [f"{self.archive}/{fullname.replace('.', '/')}"]
            if spec.submodule_search_locations is None:
                spec.submodule_search_locations = list(module.__path__)
        else:
            module.__package__ = fullname.rpartition(".")[0]
        sys.modules[fullname] = module
        _exec_restricted(module, source, origin)
        return module

    def find_module(self, fullname: str, path=None):
        try:
            self._get_source(fullname)
        except ZipImportError:
            return None
        return self

    def get_source(self, fullname: str) -> str | None:
        try:
            source, _, _ = self._get_source(fullname)
        except ZipImportError:
            return None
        return source

    def _get_source(self, fullname: str) -> tuple[str, bool, str]:
        module_path = fullname.replace(".", "/")
        if self._prefix:
            module_path = f"{self._prefix}/{module_path}"
        mod_file = f"{module_path}.py"
        pkg_file = f"{module_path}/__init__.py"
        try:
            zf = _get_zipfile(self.archive)
            try:
                try:
                    data = zf.read(mod_file)
                    is_package = False
                    inner_path = mod_file
                except KeyError:
                    data = zf.read(pkg_file)
                    is_package = True
                    inner_path = pkg_file
            except KeyError as exc:
                raise ZipImportError(str(exc)) from exc
        except ZipImportError:
            raise
        except (BadZipFile, OSError) as exc:
            raise ZipImportError(str(exc)) from exc
        try:
            source = data.decode("utf-8", errors="surrogateescape")
        except Exception:
            source = data.decode("utf-8", errors="replace")
        return source, is_package, inner_path


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


def _get_zipfile(archive: str) -> ZipFile:
    entry = _ZIP_CACHE.get(archive)
    st = os.stat(archive)
    mtime = st.st_mtime
    size = st.st_size
    if entry is not None and entry.mtime == mtime and entry.size == size:
        return entry.zf
    zf = ZipFile(archive, "r")
    _ZIP_CACHE[archive] = _ZipCacheEntry(archive, zf, mtime, size)
    return zf
