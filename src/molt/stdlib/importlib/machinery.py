"""Minimal importlib.machinery support for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


# Loader identity and behavior belong to the runtime, so generated module
# metadata can use them before this facade (or any filesystem facade) imports.
_MoltLoader, BuiltinImporter, FrozenImporter = _require_intrinsic(
    "molt_importlib_compiled_loader_types"
)()


_LoaderBasics = _MoltLoader
_MOLT_LOADER = _require_intrinsic("molt_importlib_compiled_loader")()

# The runtime owns the default finders: sys.meta_path and sys.path_hooks hold
# them from bootstrap, so this facade only re-exports their identities.
PathFinder, FileFinder = _require_intrinsic("molt_importlib_path_finder_types")()
_MOLT_IMPORTLIB_NAMESPACE_PATH_EPOCH = _require_intrinsic(
    "molt_importlib_namespace_path_epoch"
)


SOURCE_SUFFIXES, BYTECODE_SUFFIXES = _require_intrinsic("molt_importlib_suffixes")()
DEBUG_BYTECODE_SUFFIXES = OPTIMIZED_BYTECODE_SUFFIXES = BYTECODE_SUFFIXES
import sys as _sys


# The runtime owns the one ModuleSpec class. Runtime importlib consumers and
# extension initialization construct this same type without this facade.
ModuleSpec = _require_intrinsic("molt_importlib_module_spec_type")()


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
        _run_compiled_loader_exec(
            0,
            self.name,
            self,
            module,
            self.path,
            None,
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
        _run_compiled_loader_exec(
            1,
            self.name,
            self,
            module,
            self.archive_path,
            self.inner_path,
        )


class ExtensionFileLoader(_FileLoader):
    def __init__(self, fullname: str, path: str) -> None:
        self.name = fullname
        self.path = str(path)

    def __repr__(self) -> str:
        return f"<MoltExtensionFileLoader name={self.name!r} path={self.path!r}>"

    def get_filename(self, _fullname: str | None = None) -> str:
        return self.path

    def create_module(self, spec: ModuleSpec):
        module_name = spec.name
        modules = getattr(_sys, "modules", None)
        previous = (
            modules.get(module_name, _MISSING_SYS_MODULE)
            if isinstance(modules, dict)
            else _MISSING_SYS_MODULE
        )
        try:
            _ensure_intrinsics()
            _restore_sys_modules_entry(modules, module_name, previous)
            module = _MOLT_IMPORTLIB_EXTENSION_LOADER_CREATE_MODULE(
                module_name, self.path, spec
            )
            _raise_loader_pending_exception()
            return module
        finally:
            _restore_sys_modules_entry(modules, module_name, previous)

    def exec_module(self, module) -> None:
        _run_compiled_loader_exec(
            2,
            self.name,
            self,
            module,
            self.path,
            None,
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
        _run_compiled_loader_exec(
            3,
            self.name,
            self,
            module,
            self.path,
            None,
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


_MISSING_SYS_MODULE = object()


def _restore_sys_modules_entry(modules, module_name: str, previous) -> None:
    if not isinstance(modules, dict):
        return
    if previous is _MISSING_SYS_MODULE:
        modules.pop(module_name, None)
    else:
        modules[module_name] = previous


def _run_compiled_loader_exec(
    kind: int,
    module_name: str,
    loader,
    module,
    path: str,
    inner_path: str | None,
) -> None:
    modules = getattr(_sys, "modules", None)
    previous = (
        modules.get(module_name, _MISSING_SYS_MODULE)
        if isinstance(modules, dict)
        else _MISSING_SYS_MODULE
    )
    try:
        _ensure_intrinsics()
        # Intrinsic bootstrap may initialize statically admitted modules.
        # Restore the caller's exact mapping before executing the requested
        # body; direct Loader.exec_module does not own sys.modules publication.
        _restore_sys_modules_entry(modules, module_name, previous)
        # Like CPython's Loader.exec_module, these run the body and set no
        # module metadata: module_from_spec owns _init_module_attrs.
        if kind == 0:
            result = _MOLT_IMPORTLIB_SOURCEFILELOADER_EXEC_MODULE(loader, module, path)
        elif kind == 1:
            result = _MOLT_IMPORTLIB_ZIP_SOURCE_LOADER_EXEC_MODULE(
                loader, module, path, inner_path
            )
        elif kind == 2:
            result = _MOLT_IMPORTLIB_EXTENSION_LOADER_EXEC_MODULE(loader, module, path)
        else:
            result = _MOLT_IMPORTLIB_SOURCELESS_LOADER_EXEC_MODULE(loader, module, path)
        _check_loader_exec_result(result)
    finally:
        _restore_sys_modules_entry(modules, module_name, previous)


def _check_loader_exec_result(result) -> None:
    _raise_loader_pending_exception()
    if result is not None:
        raise RuntimeError(
            "invalid importlib loader execution intrinsic result: expected None"
        )


def _raise_loader_pending_exception() -> None:
    exc = _MOLT_EXCEPTION_LAST_PENDING()
    if exc is not None:
        cleared = _MOLT_EXCEPTION_CLEAR()
        if cleared is not None:
            raise RuntimeError(
                "invalid exception clear intrinsic result: expected None"
            )
        if isinstance(exc, BaseException):
            raise exc
        raise RuntimeError("importlib loader execution failed")


class _NamespacePath:
    """A namespace package's ``__path__``: CPython's
    ``importlib._bootstrap_external._NamespacePath``.

    It reads the parent's path (``sys.path`` for a top-level package) and asks
    ``path_finder`` for the portions again when that path changes or
    ``PathFinder.invalidate_caches`` ran.
    """

    def __init__(self, name, path, path_finder):
        self._name = name
        self._path = path
        self._last_parent_path = tuple(self._get_parent_path())
        # PathFinder.invalidate_caches advances the runtime epoch (bpo-45703).
        self._last_epoch = _MOLT_IMPORTLIB_NAMESPACE_PATH_EPOCH()
        self._path_finder = path_finder

    def _find_parent_path_names(self):
        """Return (parent-module-name, parent-path-attr-name)."""
        parent, dot, _me = self._name.rpartition(".")
        if dot == "":
            return "sys", "path"
        return parent, "__path__"

    def _get_parent_path(self):
        parent_module_name, path_attr_name = self._find_parent_path_names()
        return getattr(_sys.modules[parent_module_name], path_attr_name)

    def _recalculate(self):
        parent_path = tuple(self._get_parent_path())
        epoch = _MOLT_IMPORTLIB_NAMESPACE_PATH_EPOCH()
        if parent_path != self._last_parent_path or epoch != self._last_epoch:
            spec = self._path_finder(self._name, parent_path)
            # A returned loader changes nothing, but the new parent path is kept.
            if spec is not None and spec.loader is None:
                if spec.submodule_search_locations:
                    self._path = spec.submodule_search_locations
            self._last_parent_path = parent_path
            self._last_epoch = epoch
        return self._path

    def __iter__(self):
        return iter(self._recalculate())

    def __getitem__(self, index):
        return self._recalculate()[index]

    def __setitem__(self, index, path):
        self._path[index] = path

    def __len__(self):
        return len(self._recalculate())

    def __repr__(self):
        return f"_NamespacePath({self._path!r})"

    def __contains__(self, item):
        return item in self._recalculate()

    def append(self, item):
        self._path.append(item)


# The runtime constructs file-hit specs from these classes and wraps namespace
# portions in _NamespacePath. The declaration names this body's own classes, so
# a replaced sys.modules entry cannot substitute them.
_MOLT_IMPORTLIB_FILE_LOADER_TYPES_DECLARE = _require_intrinsic(
    "molt_importlib_file_loader_types_declare"
)
_MOLT_IMPORTLIB_FILE_LOADER_TYPES_DECLARE(globals())


class NamespaceLoader:
    def __init__(self, name: str, path, path_finder) -> None:
        self._path = _NamespacePath(name, path, path_finder)

    def create_module(self, _spec: ModuleSpec):
        return None

    def exec_module(self, _module) -> None:
        return None

    def is_package(self, _fullname: str) -> bool:
        return True

    def get_source(self, _fullname: str) -> str:
        return ""

    def get_resource_reader(self, fullname: str):
        _ensure_intrinsics()
        del fullname
        return _MoltResourceReader(list(self._path))


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


_MOLT_IMPORTLIB_READ_FILE = None
_MOLT_IMPORTLIB_SOURCEFILELOADER_EXEC_MODULE = None
_MOLT_IMPORTLIB_ZIP_SOURCE_LOADER_EXEC_MODULE = None
_MOLT_IMPORTLIB_EXTENSION_LOADER_CREATE_MODULE = None
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
_MOLT_EXCEPTION_LAST_PENDING = None
_MOLT_IMPORTLIB_INTRINSICS_READY = False


def _ensure_intrinsics() -> None:
    global _MOLT_IMPORTLIB_READ_FILE
    global _MOLT_IMPORTLIB_SOURCEFILELOADER_EXEC_MODULE
    global _MOLT_IMPORTLIB_ZIP_SOURCE_LOADER_EXEC_MODULE
    global _MOLT_IMPORTLIB_EXTENSION_LOADER_CREATE_MODULE
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
    global _MOLT_EXCEPTION_LAST_PENDING
    global _MOLT_IMPORTLIB_INTRINSICS_READY
    if _MOLT_IMPORTLIB_INTRINSICS_READY:
        return
    importlib_read_file = _require_intrinsic("molt_importlib_read_file")
    importlib_sourcefileloader_exec_module = _require_intrinsic(
        "molt_importlib_sourcefileloader_exec_module"
    )
    importlib_zip_source_loader_exec_module = _require_intrinsic(
        "molt_importlib_zip_source_loader_exec_module"
    )
    importlib_extension_loader_create_module = _require_intrinsic(
        "molt_importlib_extension_loader_create_module"
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
    exception_last_pending = _require_intrinsic("molt_exception_last_pending")

    _MOLT_IMPORTLIB_READ_FILE = importlib_read_file
    _MOLT_IMPORTLIB_SOURCEFILELOADER_EXEC_MODULE = (
        importlib_sourcefileloader_exec_module
    )
    _MOLT_IMPORTLIB_ZIP_SOURCE_LOADER_EXEC_MODULE = (
        importlib_zip_source_loader_exec_module
    )
    _MOLT_IMPORTLIB_EXTENSION_LOADER_CREATE_MODULE = (
        importlib_extension_loader_create_module
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
    _MOLT_EXCEPTION_LAST_PENDING = exception_last_pending
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
