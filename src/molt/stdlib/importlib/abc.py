"""Abstract base classes related to import."""

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_stdlib_probe", globals())

import abc
import warnings

from . import _bootstrap
from . import _bootstrap_external
from . import machinery
from ._abc import Loader

_MOLT_IMPORTLIB_IMPORT_OPTIONAL = _require_intrinsic(
    "molt_importlib_import_optional", globals()
)
_MOLT_IMPORTLIB_IMPORT_OR_FALLBACK = _require_intrinsic(
    "molt_importlib_import_or_fallback", globals()
)

_RESOURCE_ABC_EXPORTS = frozenset(
    (
        "ResourceReader",
        "Traversable",
        "TraversableResources",
    )
)
_resources_abc = None

_frozen_importlib = _bootstrap
_frozen_importlib_external = _MOLT_IMPORTLIB_IMPORT_OR_FALLBACK(
    "_frozen_importlib_external",
    _bootstrap_external,
)

__all__ = [
    "Loader",
    "MetaPathFinder",
    "PathEntryFinder",
    "ResourceLoader",
    "InspectLoader",
    "ExecutionLoader",
    "FileLoader",
    "SourceLoader",
]


def __getattr__(name):
    global _resources_abc
    if name in _RESOURCE_ABC_EXPORTS:
        if _resources_abc is None:
            _resources_abc = _MOLT_IMPORTLIB_IMPORT_OPTIONAL("importlib.resources.abc")
            if _resources_abc is None:
                raise AttributeError(f"module {__name__!r} has no attribute {name!r}")
        obj = getattr(_resources_abc, name)
        warnings._deprecated(f"{__name__}.{name}", remove=(3, 14))
        import sys as _abc_sys
        _abc_mod_dict = getattr(_abc_sys.modules.get(__name__), "__dict__", None) or globals()
        _abc_mod_dict[name] = obj
        return obj
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")


def _register(abstract_cls, *classes):
    for cls in classes:
        abstract_cls.register(cls)
        if _frozen_importlib is not None:
            try:
                frozen_cls = getattr(_frozen_importlib, cls.__name__)
            except AttributeError:
                frozen_cls = getattr(_frozen_importlib_external, cls.__name__)
            abstract_cls.register(frozen_cls)


class MetaPathFinder(metaclass=abc.ABCMeta):
    def invalidate_caches(self):
        return None


_register(
    MetaPathFinder,
    machinery.BuiltinImporter,
    machinery.FrozenImporter,
    machinery.PathFinder,
    machinery.WindowsRegistryFinder,
)


class PathEntryFinder(metaclass=abc.ABCMeta):
    def invalidate_caches(self):
        return None


_register(PathEntryFinder, machinery.FileFinder)


class ResourceLoader(Loader):
    @abc.abstractmethod
    def get_data(self, path):
        raise OSError


class InspectLoader(Loader):
    def is_package(self, fullname):
        raise ImportError

    def get_code(self, fullname):
        source = self.get_source(fullname)
        if source is None:
            return None
        return self.source_to_code(source)

    @abc.abstractmethod
    def get_source(self, fullname):
        raise ImportError

    @staticmethod
    def source_to_code(data, path="<string>"):
        return compile(data, path, "exec", dont_inherit=True)

    exec_module = _bootstrap_external._LoaderBasics.exec_module
    load_module = _bootstrap_external._LoaderBasics.load_module


_register(
    InspectLoader,
    machinery.BuiltinImporter,
    machinery.FrozenImporter,
    machinery.NamespaceLoader,
)


class ExecutionLoader(InspectLoader):
    @abc.abstractmethod
    def get_filename(self, fullname):
        raise ImportError

    def get_code(self, fullname):
        source = self.get_source(fullname)
        if source is None:
            return None
        try:
            path = self.get_filename(fullname)
        except ImportError:
            return self.source_to_code(source)
        return self.source_to_code(source, path)


_register(ExecutionLoader, machinery.ExtensionFileLoader)


class FileLoader(_bootstrap_external.FileLoader, ResourceLoader, ExecutionLoader):
    """Abstract base class partially implementing ResourceLoader and ExecutionLoader."""


_register(FileLoader, machinery.SourceFileLoader, machinery.SourcelessFileLoader)


class SourceLoader(_bootstrap_external.SourceLoader, ResourceLoader, ExecutionLoader):
    """Abstract base class for loading source and bytecode."""

    def path_mtime(self, path):
        if self.path_stats.__func__ is SourceLoader.path_stats:
            raise OSError
        return int(self.path_stats(path)["mtime"])

    def path_stats(self, path):
        if self.path_mtime.__func__ is SourceLoader.path_mtime:
            raise OSError
        return {"mtime": self.path_mtime(path)}

    def set_data(self, path, data):
        return None


_register(SourceLoader, machinery.SourceFileLoader)
