"""Minimal importlib.machinery support for Molt."""

from __future__ import annotations


class _MoltLoader:
    def create_module(self, _spec: "ModuleSpec"):
        return None

    def exec_module(self, module) -> None:
        _ensure_intrinsics()
        module_name = _coerce_module_name(module, self)
        previous = _drop_stale_sys_module(module_name, module)
        try:
            imported = _MOLT_MODULE_IMPORT(module_name)
            imported_dict = getattr(imported, "__dict__", None)
            if isinstance(imported, dict):
                module.__dict__.update(imported)
                return
            if isinstance(imported_dict, dict):
                module.__dict__.update(imported_dict)
                return
            raise TypeError(
                f"import returned non-module payload: {type(imported).__name__}"
            )
        except BaseException:
            _restore_sys_module(module_name, previous)
            raise

    def load_module(self, fullname: str):
        _ensure_intrinsics()
        previous = _drop_stale_sys_module(fullname)
        try:
            return _MOLT_MODULE_IMPORT(fullname)
        except BaseException:
            _restore_sys_module(fullname, previous)
            raise

    def __repr__(self) -> str:
        return "<_MoltLoader>"


class BuiltinImporter(_MoltLoader):
    def __repr__(self) -> str:
        return "<BuiltinImporter>"


class FrozenImporter(_MoltLoader):
    def __repr__(self) -> str:
        return "<FrozenImporter>"


_LoaderBasics = _MoltLoader
_MOLT_LOADER = BuiltinImporter()


class ModuleSpec:
    def __init__(
        self,
        name: str,
        loader: object | None = None,
        origin: str | None = None,
        is_package: bool | None = None,
    ) -> None:
        self.name = str(name)
        self.loader = loader
        self.origin = origin
        self.loader_state = None
        self.cached = None
        if is_package:
            self.submodule_search_locations = []
        else:
            self.submodule_search_locations = None
        self.has_location = origin is not None

    @property
    def parent(self) -> str:
        if self.submodule_search_locations is None:
            return self.name.rpartition(".")[0]
        return self.name

    def __repr__(self) -> str:
        return (
            "ModuleSpec("
            f"name={self.name!r}, "
            f"loader={self.loader!r}, "
            f"origin={self.origin!r})"
        )


SOURCE_SUFFIXES = [".py"]
BYTECODE_SUFFIXES = [".pyc"]
DEBUG_BYTECODE_SUFFIXES = [".pyc"]
OPTIMIZED_BYTECODE_SUFFIXES = [".pyc"]
import sys as _sys


def _require_intrinsic(name: str, namespace: dict[str, object] | None = None):
    from _intrinsics import require_intrinsic as _require

    return _require(name, namespace)


def _resolve_platform() -> str:
    platform_fn = _require_intrinsic("molt_sys_platform")
    platform = platform_fn()
    if not isinstance(platform, str):
        raise RuntimeError("molt_sys_platform returned invalid value")
    return platform


_platform = _resolve_platform()

if _platform == "win32":
    EXTENSION_SUFFIXES: list[str] = [".pyd", ".dll"]
elif _platform == "darwin":
    EXTENSION_SUFFIXES: list[str] = [".so", ".dylib"]
else:
    EXTENSION_SUFFIXES: list[str] = [".so"]


def all_suffixes() -> list[str]:
    return SOURCE_SUFFIXES + BYTECODE_SUFFIXES + EXTENSION_SUFFIXES


def _drop_stale_sys_module(module_name: str, module=None):
    modules = getattr(_sys, "modules", None)
    if not isinstance(modules, dict):
        return None
    existing = modules.get(module_name)
    if existing is None or existing is module:
        return None
    del modules[module_name]
    return existing


def _restore_sys_module(module_name: str, previous) -> None:
    if previous is None:
        return
    modules = getattr(_sys, "modules", None)
    if not isinstance(modules, dict) or module_name in modules:
        return
    modules[module_name] = previous


class _FileLoader:
    def __init__(self, fullname: str, path: str) -> None:
        self.name = fullname
        self.path = str(path)

    def get_filename(self, _fullname: str | None = None) -> str:
        return self.path

    def get_data(self, path: str) -> bytes:
        _ensure_intrinsics()
        payload = _MOLT_IMPORTLIB_READ_FILE(path)
        if not isinstance(payload, bytes):
            raise RuntimeError("invalid importlib read payload: bytes expected")
        return payload

    def create_module(self, _spec: ModuleSpec):
        return None

    def load_module(self, fullname: str | None = None):
        """Load and return the module identified by ``fullname``.

        Mirrors CPython's ``importlib._bootstrap._load_module_shim`` for the
        Molt file-loader hierarchy: build a ``ModuleSpec`` from this loader and
        the loader's ``path``, materialise a fresh module object via
        ``module_from_spec``, register it in ``sys.modules``, and run
        ``exec_module``.  Restores the previous ``sys.modules`` entry on
        failure so partially initialised modules are not left visible.
        """
        _ensure_intrinsics()
        if fullname is None:
            fullname = self.name
        else:
            fullname = str(fullname)
        # Lazy import to avoid the ``importlib.util``/``importlib.machinery``
        # bootstrap cycle: ``util`` re-imports ``machinery``.
        from . import util as _util

        spec = _util.spec_from_file_location(fullname, self.path, loader=self)
        if spec is None:
            raise ImportError(
                f"could not build spec for {fullname!r} from {self.path!r}"
            )
        return _MOLT_IMPORTLIB_LOAD_MODULE_FROM_SPEC(self, fullname, spec)


class _SourceLoader(_FileLoader):
    pass


FileLoader = _FileLoader
SourceLoader = _SourceLoader


class SourceFileLoader(_SourceLoader):
    def __init__(self, fullname: str, path: str) -> None:
        self.name = fullname
        self.path = str(path)

    def __repr__(self) -> str:
        return f"<MoltSourceFileLoader name={self.name!r} path={self.path!r}>"

    def get_filename(self, _fullname: str | None = None) -> str:
        return self.path

    def get_data(self, path: str) -> bytes:
        _ensure_intrinsics()
        payload = _MOLT_IMPORTLIB_READ_FILE(path)
        if not isinstance(payload, bytes):
            raise RuntimeError("invalid importlib read payload: bytes expected")
        return payload

    def get_resource_reader(self, fullname: str):
        _ensure_intrinsics()
        if fullname != self.name:
            return None
        package_root = _package_root_from_origin(self.path)
        if package_root is None:
            return None
        return _MoltResourceReader([package_root])

    def create_module(self, _spec: ModuleSpec):
        return None

    def exec_module(self, module) -> None:
        _ensure_intrinsics()
        _check_loader_exec_result(
            _MOLT_IMPORTLIB_SOURCEFILELOADER_EXEC_MODULE(
                self, module, self.path, ModuleSpec
            )
        )


class _ZipSourceLoader:
    def __init__(self, fullname: str, archive_path: str, inner_path: str) -> None:
        self.name = fullname
        self.archive_path = str(archive_path)
        self.inner_path = str(inner_path)
        self.path = f"{self.archive_path}/{self.inner_path}"

    def __repr__(self) -> str:
        return (
            "<MoltZipSourceLoader "
            f"name={self.name!r} archive={self.archive_path!r} inner={self.inner_path!r}>"
        )

    def get_filename(self, _fullname: str | None = None) -> str:
        return self.path

    def get_resource_reader(self, fullname: str):
        _ensure_intrinsics()
        if fullname != self.name:
            return None
        package_root = _package_root_from_origin(self.path)
        if package_root is None:
            return None
        return _MoltResourceReader([package_root])

    def create_module(self, _spec: ModuleSpec):
        return None

    def exec_module(self, module) -> None:
        _ensure_intrinsics()
        _check_loader_exec_result(
            _MOLT_IMPORTLIB_ZIP_SOURCE_LOADER_EXEC_MODULE(
                self, module, self.archive_path, self.inner_path, ModuleSpec
            )
        )


class ExtensionFileLoader(_FileLoader):
    def __init__(self, fullname: str, path: str) -> None:
        self.name = fullname
        self.path = str(path)

    def __repr__(self) -> str:
        return f"<MoltExtensionFileLoader name={self.name!r} path={self.path!r}>"

    def get_filename(self, _fullname: str | None = None) -> str:
        return self.path

    def create_module(self, _spec: ModuleSpec):
        return None

    def exec_module(self, module) -> None:
        _ensure_intrinsics()
        _check_loader_exec_result(
            _MOLT_IMPORTLIB_EXTENSION_LOADER_EXEC_MODULE(
                self, module, self.path, ModuleSpec
            )
        )


class SourcelessFileLoader(_FileLoader):
    def __init__(self, fullname: str, path: str) -> None:
        self.name = fullname
        self.path = str(path)

    def __repr__(self) -> str:
        return f"<MoltSourcelessFileLoader name={self.name!r} path={self.path!r}>"

    def get_filename(self, _fullname: str | None = None) -> str:
        return self.path

    def get_resource_reader(self, fullname: str):
        _ensure_intrinsics()
        if fullname != self.name:
            return None
        package_root = _package_root_from_origin(self.path)
        if package_root is None:
            return None
        return _MoltResourceReader([package_root])

    def create_module(self, _spec: ModuleSpec):
        return None

    def exec_module(self, module) -> None:
        _ensure_intrinsics()
        _check_loader_exec_result(
            _MOLT_IMPORTLIB_SOURCELESS_LOADER_EXEC_MODULE(
                self, module, self.path, ModuleSpec
            )
        )


class _MoltResourceReader:
    def __init__(self, roots: list[str]) -> None:
        unique: list[str] = []
        for root in roots:
            if isinstance(root, str) and root and root not in unique:
                unique.append(root)
        self._roots = unique

    def molt_roots(self) -> tuple[str, ...]:
        return tuple(self._roots)

    def files(self) -> str:
        if not self._roots:
            raise FileNotFoundError("resource root unavailable")
        return self._roots[0]

    def resource_path(self, resource: str) -> str:
        name = _validate_resource_name(resource)
        path = _MOLT_IMPORTLIB_RESOURCES_READER_RESOURCE_PATH_FROM_ROOTS(
            self._roots, name
        )
        if path is None:
            raise FileNotFoundError(name)
        if not isinstance(path, str):
            raise RuntimeError("invalid importlib resource path payload: str expected")
        return path

    def open_resource(self, resource: str):
        name = _validate_resource_name(resource)
        data = _MOLT_IMPORTLIB_RESOURCES_READER_OPEN_RESOURCE_BYTES_FROM_ROOTS(
            self._roots, name
        )
        if not isinstance(data, bytes):
            raise RuntimeError(
                "invalid importlib open resource payload: bytes expected"
            )
        import io as _io

        return _io.BytesIO(data)

    def is_resource(self, resource: str) -> bool:
        name = _validate_resource_name(resource)
        value = _MOLT_IMPORTLIB_RESOURCES_READER_IS_RESOURCE_FROM_ROOTS(
            self._roots, name
        )
        if not isinstance(value, bool):
            raise RuntimeError("invalid importlib is_resource payload: bool expected")
        return value

    def contents(self) -> list[str]:
        values = _MOLT_IMPORTLIB_RESOURCES_READER_CONTENTS_FROM_ROOTS(self._roots)
        if not isinstance(values, list) or not all(
            isinstance(entry, str) for entry in values
        ):
            raise RuntimeError("invalid importlib resource contents payload: list[str]")
        return values


def _check_loader_exec_result(result) -> None:
    if _MOLT_EXCEPTION_PENDING():
        exc = _MOLT_EXCEPTION_LAST()
        cleared = _MOLT_EXCEPTION_CLEAR()
        if cleared is not None:
            raise RuntimeError(
                "invalid exception clear intrinsic result: expected None"
            )
        if isinstance(exc, BaseException):
            raise exc
        raise RuntimeError("importlib loader execution failed")
    if result is not None:
        raise RuntimeError(
            "invalid importlib loader execution intrinsic result: expected None"
        )


class PathFinder:
    @classmethod
    def find_spec(
        cls,
        fullname: str,
        path: object | None = None,
        target: object | None = None,
    ):
        del cls, target
        _ensure_intrinsics()
        import sys as _sys

        machinery_module = _sys.modules.get(__name__)
        if machinery_module is None:
            raise RuntimeError("importlib.machinery module unavailable")
        return _MOLT_IMPORTLIB_PATHFINDER_FIND_SPEC(fullname, path, machinery_module)


class FileFinder:
    def __init__(self, path: str, *loader_details) -> None:
        self.path = str(path)
        self._loader_details = tuple(loader_details)

    def __repr__(self) -> str:
        return f"<MoltFileFinder path={self.path!r}>"

    @classmethod
    def path_hook(cls, *loader_details):
        def _path_hook(path: str):
            if not isinstance(path, str):
                raise ImportError("only str paths are supported")
            return cls(path, *loader_details)

        return _path_hook

    def find_spec(self, fullname: str, target: object | None = None):
        del target
        _ensure_intrinsics()
        import sys as _sys

        machinery_module = _sys.modules.get(__name__)
        if machinery_module is None:
            raise RuntimeError("importlib.machinery module unavailable")
        return _MOLT_IMPORTLIB_FILEFINDER_FIND_SPEC(
            fullname, self.path, machinery_module
        )

    def invalidate_caches(self) -> None:
        _ensure_intrinsics()
        result = _MOLT_IMPORTLIB_FILEFINDER_INVALIDATE(self.path)
        if result is not None:
            raise RuntimeError(
                "invalid importlib filefinder invalidate intrinsic result: expected None"
            )
        return None


class NamespaceLoader:
    def __init__(self, name: str, path, path_finder=None) -> None:
        del path_finder
        self.name = str(name)
        if isinstance(path, str):
            values = [path] if path else []
        elif isinstance(path, (list, tuple)):
            values = [entry for entry in path if isinstance(entry, str) and entry]
        else:
            values = []
        self.path = list(values)

    def __repr__(self) -> str:
        return f"<MoltNamespaceLoader name={self.name!r}>"

    def create_module(self, _spec: ModuleSpec):
        return None

    def exec_module(self, _module) -> None:
        return None

    def is_package(self, _fullname: str) -> bool:
        return True

    def get_resource_reader(self, fullname: str):
        _ensure_intrinsics()
        if fullname != self.name:
            return None
        return _MoltResourceReader(list(self.path))


class WindowsRegistryFinder:
    @classmethod
    def find_spec(
        cls,
        fullname: str,
        path: object | None = None,
        target: object | None = None,
    ):
        del cls, fullname, path, target
        return None


def _package_root_from_origin(path: str) -> str | None:
    _ensure_intrinsics()
    value = _MOLT_IMPORTLIB_PACKAGE_ROOT_FROM_ORIGIN(path)
    if value is not None and not isinstance(value, str):
        raise RuntimeError(
            "invalid importlib package root payload: str | None expected"
        )
    return value


def _validate_resource_name(resource: str) -> str:
    _ensure_intrinsics()
    value = _MOLT_IMPORTLIB_VALIDATE_RESOURCE_NAME(resource)
    if not isinstance(value, str):
        raise RuntimeError(
            "invalid importlib validate resource name payload: str expected"
        )
    return value


def _coerce_module_name(
    module,
    loader: object | None,
    spec: object | None = None,
) -> str:
    _ensure_intrinsics()
    value = _MOLT_IMPORTLIB_COERCE_MODULE_NAME(module, loader, spec)
    if not isinstance(value, str):
        raise RuntimeError("invalid importlib module name payload: str expected")
    return value


_MOLT_IMPORTLIB_READ_FILE = None
_MOLT_IMPORTLIB_COERCE_MODULE_NAME = None
_MOLT_IMPORTLIB_PATHFINDER_FIND_SPEC = None
_MOLT_IMPORTLIB_FILEFINDER_FIND_SPEC = None
_MOLT_IMPORTLIB_FILEFINDER_INVALIDATE = None
_MOLT_IMPORTLIB_SOURCEFILELOADER_EXEC_MODULE = None
_MOLT_IMPORTLIB_ZIP_SOURCE_LOADER_EXEC_MODULE = None
_MOLT_IMPORTLIB_EXTENSION_LOADER_EXEC_MODULE = None
_MOLT_IMPORTLIB_SOURCELESS_LOADER_EXEC_MODULE = None
_MOLT_IMPORTLIB_RESOURCES_READER_RESOURCE_PATH_FROM_ROOTS = None
_MOLT_IMPORTLIB_RESOURCES_READER_OPEN_RESOURCE_BYTES_FROM_ROOTS = None
_MOLT_IMPORTLIB_RESOURCES_READER_IS_RESOURCE_FROM_ROOTS = None
_MOLT_IMPORTLIB_RESOURCES_READER_CONTENTS_FROM_ROOTS = None
_MOLT_IMPORTLIB_PACKAGE_ROOT_FROM_ORIGIN = None
_MOLT_IMPORTLIB_VALIDATE_RESOURCE_NAME = None
_MOLT_IMPORTLIB_LOAD_MODULE_FROM_SPEC = None
_MOLT_EXCEPTION_CLEAR = None
_MOLT_EXCEPTION_LAST = None
_MOLT_EXCEPTION_PENDING = None
_MOLT_MODULE_IMPORT = None
_MOLT_IMPORTLIB_INTRINSICS_READY = False


def _ensure_intrinsics() -> None:
    global _MOLT_IMPORTLIB_READ_FILE
    global _MOLT_IMPORTLIB_COERCE_MODULE_NAME
    global _MOLT_IMPORTLIB_PATHFINDER_FIND_SPEC
    global _MOLT_IMPORTLIB_FILEFINDER_FIND_SPEC
    global _MOLT_IMPORTLIB_FILEFINDER_INVALIDATE
    global _MOLT_IMPORTLIB_SOURCEFILELOADER_EXEC_MODULE
    global _MOLT_IMPORTLIB_ZIP_SOURCE_LOADER_EXEC_MODULE
    global _MOLT_IMPORTLIB_EXTENSION_LOADER_EXEC_MODULE
    global _MOLT_IMPORTLIB_SOURCELESS_LOADER_EXEC_MODULE
    global _MOLT_IMPORTLIB_RESOURCES_READER_RESOURCE_PATH_FROM_ROOTS
    global _MOLT_IMPORTLIB_RESOURCES_READER_OPEN_RESOURCE_BYTES_FROM_ROOTS
    global _MOLT_IMPORTLIB_RESOURCES_READER_IS_RESOURCE_FROM_ROOTS
    global _MOLT_IMPORTLIB_RESOURCES_READER_CONTENTS_FROM_ROOTS
    global _MOLT_IMPORTLIB_PACKAGE_ROOT_FROM_ORIGIN
    global _MOLT_IMPORTLIB_VALIDATE_RESOURCE_NAME
    global _MOLT_IMPORTLIB_LOAD_MODULE_FROM_SPEC
    global _MOLT_EXCEPTION_CLEAR
    global _MOLT_EXCEPTION_LAST
    global _MOLT_EXCEPTION_PENDING
    global _MOLT_MODULE_IMPORT
    global _MOLT_IMPORTLIB_INTRINSICS_READY
    if _MOLT_IMPORTLIB_INTRINSICS_READY:
        return
    _require_intrinsic("molt_stdlib_probe")
    importlib_read_file = _require_intrinsic("molt_importlib_read_file")
    importlib_coerce_module_name = _require_intrinsic(
        "molt_importlib_coerce_module_name"
    )
    importlib_pathfinder_find_spec = _require_intrinsic(
        "molt_importlib_pathfinder_find_spec"
    )
    importlib_filefinder_find_spec = _require_intrinsic(
        "molt_importlib_filefinder_find_spec"
    )
    importlib_filefinder_invalidate = _require_intrinsic(
        "molt_importlib_filefinder_invalidate"
    )
    importlib_sourcefileloader_exec_module = _require_intrinsic(
        "molt_importlib_sourcefileloader_exec_module"
    )
    importlib_zip_source_loader_exec_module = _require_intrinsic(
        "molt_importlib_zip_source_loader_exec_module"
    )
    importlib_extension_loader_exec_module = _require_intrinsic(
        "molt_importlib_extension_loader_exec_module"
    )
    importlib_sourceless_loader_exec_module = _require_intrinsic(
        "molt_importlib_sourceless_loader_exec_module"
    )
    resources_reader_resource_path_from_roots = _require_intrinsic(
        "molt_importlib_resources_reader_resource_path_from_roots"
    )
    resources_reader_open_resource_bytes_from_roots = _require_intrinsic(
        "molt_importlib_resources_reader_open_resource_bytes_from_roots"
    )
    resources_reader_is_resource_from_roots = _require_intrinsic(
        "molt_importlib_resources_reader_is_resource_from_roots"
    )
    resources_reader_contents_from_roots = _require_intrinsic(
        "molt_importlib_resources_reader_contents_from_roots"
    )
    importlib_package_root_from_origin = _require_intrinsic(
        "molt_importlib_package_root_from_origin"
    )
    importlib_validate_resource_name = _require_intrinsic(
        "molt_importlib_validate_resource_name"
    )
    importlib_load_module_from_spec = _require_intrinsic(
        "molt_importlib_load_module_from_spec"
    )
    exception_clear = _require_intrinsic("molt_exception_clear")
    exception_last = _require_intrinsic("molt_exception_last")
    exception_pending = _require_intrinsic("molt_exception_pending")
    module_import = _require_intrinsic("molt_module_import")

    _MOLT_IMPORTLIB_READ_FILE = importlib_read_file
    _MOLT_IMPORTLIB_COERCE_MODULE_NAME = importlib_coerce_module_name
    _MOLT_IMPORTLIB_PATHFINDER_FIND_SPEC = importlib_pathfinder_find_spec
    _MOLT_IMPORTLIB_FILEFINDER_FIND_SPEC = importlib_filefinder_find_spec
    _MOLT_IMPORTLIB_FILEFINDER_INVALIDATE = importlib_filefinder_invalidate
    _MOLT_IMPORTLIB_SOURCEFILELOADER_EXEC_MODULE = (
        importlib_sourcefileloader_exec_module
    )
    _MOLT_IMPORTLIB_ZIP_SOURCE_LOADER_EXEC_MODULE = (
        importlib_zip_source_loader_exec_module
    )
    _MOLT_IMPORTLIB_EXTENSION_LOADER_EXEC_MODULE = importlib_extension_loader_exec_module
    _MOLT_IMPORTLIB_SOURCELESS_LOADER_EXEC_MODULE = (
        importlib_sourceless_loader_exec_module
    )
    _MOLT_IMPORTLIB_RESOURCES_READER_RESOURCE_PATH_FROM_ROOTS = (
        resources_reader_resource_path_from_roots
    )
    _MOLT_IMPORTLIB_RESOURCES_READER_OPEN_RESOURCE_BYTES_FROM_ROOTS = (
        resources_reader_open_resource_bytes_from_roots
    )
    _MOLT_IMPORTLIB_RESOURCES_READER_IS_RESOURCE_FROM_ROOTS = (
        resources_reader_is_resource_from_roots
    )
    _MOLT_IMPORTLIB_RESOURCES_READER_CONTENTS_FROM_ROOTS = (
        resources_reader_contents_from_roots
    )
    _MOLT_IMPORTLIB_PACKAGE_ROOT_FROM_ORIGIN = importlib_package_root_from_origin
    _MOLT_IMPORTLIB_VALIDATE_RESOURCE_NAME = importlib_validate_resource_name
    _MOLT_IMPORTLIB_LOAD_MODULE_FROM_SPEC = importlib_load_module_from_spec
    _MOLT_EXCEPTION_CLEAR = exception_clear
    _MOLT_EXCEPTION_LAST = exception_last
    _MOLT_EXCEPTION_PENDING = exception_pending
    _MOLT_MODULE_IMPORT = module_import
    _MOLT_IMPORTLIB_INTRINSICS_READY = True


__all__ = [
    "BYTECODE_SUFFIXES",
    "BuiltinImporter",
    "DEBUG_BYTECODE_SUFFIXES",
    "EXTENSION_SUFFIXES",
    "ExtensionFileLoader",
    "FileLoader",
    "FileFinder",
    "FrozenImporter",
    "ModuleSpec",
    "NamespaceLoader",
    "OPTIMIZED_BYTECODE_SUFFIXES",
    "PathFinder",
    "SOURCE_SUFFIXES",
    "SourceLoader",
    "SourceFileLoader",
    "SourcelessFileLoader",
    "WindowsRegistryFinder",
    "all_suffixes",
]
