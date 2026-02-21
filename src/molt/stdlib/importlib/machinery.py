"""Minimal importlib.machinery support for Molt."""

from __future__ import annotations


class _MoltLoader:
    def create_module(self, _spec: "ModuleSpec"):
        return None

    def exec_module(self, module) -> None:
        _ensure_intrinsics()
        module_name = _coerce_module_name(module, self)
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

    def load_module(self, fullname: str):
        _ensure_intrinsics()
        return _MOLT_MODULE_IMPORT(fullname)

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
EXTENSION_SUFFIXES: list[str] = []


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
        path = self.path
        module_name = _coerce_module_name(module, self)
        spec_has_locations = _module_spec_is_package(module)
        payload = _source_exec_payload(module_name, path, bool(spec_has_locations))
        source = payload["source"]
        is_package = payload["is_package"]
        module_package = payload["module_package"]
        package_root = payload["package_root"]
        if not isinstance(source, str):
            raise RuntimeError("invalid importlib source exec payload: source")
        if not isinstance(is_package, bool):
            raise RuntimeError("invalid importlib source exec payload: is_package")
        if not isinstance(module_package, str):
            raise RuntimeError("invalid importlib source exec payload: module_package")
        if package_root is not None and not isinstance(package_root, str):
            raise RuntimeError("invalid importlib source exec payload: package_root")
        _set_module_state(
            module,
            module_name=module_name,
            loader=self,
            origin=path,
            is_package=is_package,
            module_package=module_package,
            package_root=package_root,
        )
        _exec_restricted(module, source, path)
        _stabilize_module_state_after_exec(
            module,
            loader=self,
            origin=path,
            is_package=is_package,
            module_package=module_package,
            package_root=package_root,
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
        module_name = _coerce_module_name(module, self)
        spec_has_locations = _module_spec_is_package(module)
        payload = _zip_source_exec_payload(
            module_name,
            self.archive_path,
            self.inner_path,
            bool(spec_has_locations),
        )
        source = payload["source"]
        origin = payload["origin"]
        is_package = payload["is_package"]
        module_package = payload["module_package"]
        package_root = payload["package_root"]
        if not isinstance(source, str):
            raise RuntimeError("invalid importlib zip source exec payload: source")
        if not isinstance(origin, str):
            raise RuntimeError("invalid importlib zip source exec payload: origin")
        if not isinstance(is_package, bool):
            raise RuntimeError("invalid importlib zip source exec payload: is_package")
        if not isinstance(module_package, str):
            raise RuntimeError(
                "invalid importlib zip source exec payload: module_package"
            )
        if package_root is not None and not isinstance(package_root, str):
            raise RuntimeError(
                "invalid importlib zip source exec payload: package_root"
            )
        _set_module_state(
            module,
            module_name=module_name,
            loader=self,
            origin=origin,
            is_package=is_package,
            module_package=module_package,
            package_root=package_root,
        )
        _exec_restricted(module, source, origin)
        _stabilize_module_state_after_exec(
            module,
            loader=self,
            origin=origin,
            is_package=is_package,
            module_package=module_package,
            package_root=package_root,
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
        module_name = _coerce_module_name(module, self)
        spec_has_locations = _module_spec_is_package(module)
        payload = _extension_loader_payload(
            module_name,
            self.path,
            bool(spec_has_locations),
        )
        is_package = payload["is_package"]
        module_package = payload["module_package"]
        package_root = payload["package_root"]
        if not isinstance(is_package, bool):
            raise RuntimeError("invalid importlib extension loader payload: is_package")
        if not isinstance(module_package, str):
            raise RuntimeError(
                "invalid importlib extension loader payload: module_package"
            )
        if package_root is not None and not isinstance(package_root, str):
            raise RuntimeError(
                "invalid importlib extension loader payload: package_root"
            )
        _set_module_state(
            module,
            module_name=module_name,
            loader=self,
            origin=self.path,
            is_package=bool(is_package),
            module_package=module_package,
            package_root=package_root,
        )
        result = _MOLT_IMPORTLIB_EXEC_EXTENSION(module.__dict__, module_name, self.path)
        if result is not None:
            raise RuntimeError(
                "invalid importlib extension execution intrinsic result: expected None"
            )
        _stabilize_module_state_after_exec(
            module,
            loader=self,
            origin=self.path,
            is_package=bool(is_package),
            module_package=module_package,
            package_root=package_root,
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
        if _is_archive_member_path(self.path):
            raise NotADirectoryError(self.path)
        module_name = _coerce_module_name(module, self)
        spec_has_locations = _module_spec_is_package(module)
        payload = _sourceless_loader_payload(
            module_name,
            self.path,
            bool(spec_has_locations),
        )
        is_package = payload["is_package"]
        module_package = payload["module_package"]
        package_root = payload["package_root"]
        if not isinstance(is_package, bool):
            raise RuntimeError(
                "invalid importlib sourceless loader payload: is_package"
            )
        if not isinstance(module_package, str):
            raise RuntimeError(
                "invalid importlib sourceless loader payload: module_package"
            )
        if package_root is not None and not isinstance(package_root, str):
            raise RuntimeError(
                "invalid importlib sourceless loader payload: package_root"
            )
        _set_module_state(
            module,
            module_name=module_name,
            loader=self,
            origin=self.path,
            is_package=bool(is_package),
            module_package=module_package,
            package_root=package_root,
        )
        result = _MOLT_IMPORTLIB_EXEC_SOURCELESS(
            module.__dict__, module_name, self.path
        )
        if result is not None:
            raise RuntimeError(
                "invalid importlib sourceless execution intrinsic result: expected None"
            )
        _stabilize_module_state_after_exec(
            module,
            loader=self,
            origin=self.path,
            is_package=bool(is_package),
            module_package=module_package,
            package_root=package_root,
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


def _set_module_state(
    module,
    *,
    module_name: str,
    loader: object,
    origin: str,
    is_package: bool,
    module_package: str,
    package_root: str | None,
) -> None:
    _ensure_intrinsics()
    result = _MOLT_IMPORTLIB_SET_MODULE_STATE(
        module,
        module_name,
        loader,
        origin,
        is_package,
        module_package,
        package_root,
        ModuleSpec,
    )
    if result is not None:
        raise RuntimeError(
            "invalid importlib set module state intrinsic result: expected None"
        )


def _is_archive_member_path(path: str) -> bool:
    _ensure_intrinsics()
    value = _MOLT_IMPORTLIB_PATH_IS_ARCHIVE_MEMBER(path)
    if not isinstance(value, bool):
        raise RuntimeError(
            "invalid importlib archive member path payload: bool expected"
        )
    return value


def _stabilize_module_state_after_exec(
    module,
    *,
    loader: object,
    origin: str,
    is_package: bool,
    module_package: str,
    package_root: str | None,
) -> None:
    _ensure_intrinsics()
    result = _MOLT_IMPORTLIB_STABILIZE_MODULE_STATE(
        module,
        loader,
        origin,
        is_package,
        module_package,
        package_root,
    )
    if result is not None:
        raise RuntimeError(
            "invalid importlib stabilize module state intrinsic result: expected None"
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


def _exec_restricted(module, source: str, filename: str) -> None:
    _ensure_intrinsics()
    result = _MOLT_IMPORTLIB_EXEC_RESTRICTED_SOURCE(module.__dict__, source, filename)
    if result is not None:
        raise RuntimeError(
            "invalid importlib source execution intrinsic result: expected None"
        )
    cleared = _MOLT_EXCEPTION_CLEAR()
    if cleared is not None:
        raise RuntimeError("invalid exception clear intrinsic result: expected None")


def _module_spec_is_package(module: object) -> bool:
    _ensure_intrinsics()
    value = _MOLT_IMPORTLIB_MODULE_SPEC_IS_PACKAGE(module)
    if not isinstance(value, bool):
        raise RuntimeError(
            "invalid importlib module spec package payload: bool expected"
        )
    return value


def _source_exec_payload(
    module_name: str, path: str, spec_is_package: bool
) -> dict[str, object]:
    _ensure_intrinsics()
    payload = _MOLT_IMPORTLIB_SOURCE_EXEC_PAYLOAD(module_name, path, spec_is_package)
    if not isinstance(payload, dict):
        raise RuntimeError("invalid importlib source exec payload: dict expected")
    return payload


def _zip_source_exec_payload(
    module_name: str, archive_path: str, inner_path: str, spec_is_package: bool
) -> dict[str, object]:
    _ensure_intrinsics()
    payload = _MOLT_IMPORTLIB_ZIP_SOURCE_EXEC_PAYLOAD(
        module_name, archive_path, inner_path, spec_is_package
    )
    if not isinstance(payload, dict):
        raise RuntimeError("invalid importlib zip source exec payload: dict expected")
    return payload


def _extension_loader_payload(
    module_name: str, path: str, spec_is_package: bool
) -> dict[str, object]:
    _ensure_intrinsics()
    payload = _MOLT_IMPORTLIB_EXTENSION_LOADER_PAYLOAD(
        module_name, path, spec_is_package
    )
    if not isinstance(payload, dict):
        raise RuntimeError("invalid importlib extension loader payload: dict expected")
    return payload


def _sourceless_loader_payload(
    module_name: str, path: str, spec_is_package: bool
) -> dict[str, object]:
    _ensure_intrinsics()
    payload = _MOLT_IMPORTLIB_SOURCELESS_LOADER_PAYLOAD(
        module_name, path, spec_is_package
    )
    if not isinstance(payload, dict):
        raise RuntimeError("invalid importlib sourceless loader payload: dict expected")
    return payload


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


def _require_intrinsic(name: str, namespace: dict[str, object] | None = None):
    from _intrinsics import require_intrinsic as _require

    return _require(name, namespace)


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


_MOLT_IMPORTLIB_SOURCE_EXEC_PAYLOAD = None
_MOLT_IMPORTLIB_ZIP_SOURCE_EXEC_PAYLOAD = None
_MOLT_IMPORTLIB_READ_FILE = None
_MOLT_IMPORTLIB_COERCE_MODULE_NAME = None
_MOLT_IMPORTLIB_PATHFINDER_FIND_SPEC = None
_MOLT_IMPORTLIB_FILEFINDER_FIND_SPEC = None
_MOLT_IMPORTLIB_EXEC_RESTRICTED_SOURCE = None
_MOLT_IMPORTLIB_EXEC_EXTENSION = None
_MOLT_IMPORTLIB_EXEC_SOURCELESS = None
_MOLT_IMPORTLIB_EXTENSION_LOADER_PAYLOAD = None
_MOLT_IMPORTLIB_SOURCELESS_LOADER_PAYLOAD = None
_MOLT_IMPORTLIB_MODULE_SPEC_IS_PACKAGE = None
_MOLT_IMPORTLIB_RESOURCES_READER_RESOURCE_PATH_FROM_ROOTS = None
_MOLT_IMPORTLIB_RESOURCES_READER_OPEN_RESOURCE_BYTES_FROM_ROOTS = None
_MOLT_IMPORTLIB_RESOURCES_READER_IS_RESOURCE_FROM_ROOTS = None
_MOLT_IMPORTLIB_RESOURCES_READER_CONTENTS_FROM_ROOTS = None
_MOLT_IMPORTLIB_PATH_IS_ARCHIVE_MEMBER = None
_MOLT_IMPORTLIB_PACKAGE_ROOT_FROM_ORIGIN = None
_MOLT_IMPORTLIB_VALIDATE_RESOURCE_NAME = None
_MOLT_IMPORTLIB_SET_MODULE_STATE = None
_MOLT_IMPORTLIB_STABILIZE_MODULE_STATE = None
_MOLT_EXCEPTION_CLEAR = None
_MOLT_MODULE_IMPORT = None


def _ensure_intrinsics() -> None:
    global _MOLT_IMPORTLIB_SOURCE_EXEC_PAYLOAD
    global _MOLT_IMPORTLIB_ZIP_SOURCE_EXEC_PAYLOAD
    global _MOLT_IMPORTLIB_READ_FILE
    global _MOLT_IMPORTLIB_COERCE_MODULE_NAME
    global _MOLT_IMPORTLIB_PATHFINDER_FIND_SPEC
    global _MOLT_IMPORTLIB_FILEFINDER_FIND_SPEC
    global _MOLT_IMPORTLIB_EXEC_RESTRICTED_SOURCE
    global _MOLT_IMPORTLIB_EXEC_EXTENSION
    global _MOLT_IMPORTLIB_EXEC_SOURCELESS
    global _MOLT_IMPORTLIB_EXTENSION_LOADER_PAYLOAD
    global _MOLT_IMPORTLIB_SOURCELESS_LOADER_PAYLOAD
    global _MOLT_IMPORTLIB_MODULE_SPEC_IS_PACKAGE
    global _MOLT_IMPORTLIB_RESOURCES_READER_RESOURCE_PATH_FROM_ROOTS
    global _MOLT_IMPORTLIB_RESOURCES_READER_OPEN_RESOURCE_BYTES_FROM_ROOTS
    global _MOLT_IMPORTLIB_RESOURCES_READER_IS_RESOURCE_FROM_ROOTS
    global _MOLT_IMPORTLIB_RESOURCES_READER_CONTENTS_FROM_ROOTS
    global _MOLT_IMPORTLIB_PATH_IS_ARCHIVE_MEMBER
    global _MOLT_IMPORTLIB_PACKAGE_ROOT_FROM_ORIGIN
    global _MOLT_IMPORTLIB_VALIDATE_RESOURCE_NAME
    global _MOLT_IMPORTLIB_SET_MODULE_STATE
    global _MOLT_IMPORTLIB_STABILIZE_MODULE_STATE
    global _MOLT_EXCEPTION_CLEAR
    global _MOLT_MODULE_IMPORT
    if _MOLT_IMPORTLIB_SOURCE_EXEC_PAYLOAD is not None:
        return
    _require_intrinsic("molt_stdlib_probe", globals())
    _MOLT_IMPORTLIB_SOURCE_EXEC_PAYLOAD = _require_intrinsic(
        "molt_importlib_source_exec_payload", globals()
    )
    _MOLT_IMPORTLIB_ZIP_SOURCE_EXEC_PAYLOAD = _require_intrinsic(
        "molt_importlib_zip_source_exec_payload", globals()
    )
    _MOLT_IMPORTLIB_READ_FILE = _require_intrinsic(
        "molt_importlib_read_file", globals()
    )
    _MOLT_IMPORTLIB_COERCE_MODULE_NAME = _require_intrinsic(
        "molt_importlib_coerce_module_name", globals()
    )
    _MOLT_IMPORTLIB_PATHFINDER_FIND_SPEC = _require_intrinsic(
        "molt_importlib_pathfinder_find_spec", globals()
    )
    _MOLT_IMPORTLIB_FILEFINDER_FIND_SPEC = _require_intrinsic(
        "molt_importlib_filefinder_find_spec", globals()
    )
    _MOLT_IMPORTLIB_EXEC_RESTRICTED_SOURCE = _require_intrinsic(
        "molt_importlib_exec_restricted_source", globals()
    )
    _MOLT_IMPORTLIB_EXEC_EXTENSION = _require_intrinsic(
        "molt_importlib_exec_extension", globals()
    )
    _MOLT_IMPORTLIB_EXEC_SOURCELESS = _require_intrinsic(
        "molt_importlib_exec_sourceless", globals()
    )
    _MOLT_IMPORTLIB_EXTENSION_LOADER_PAYLOAD = _require_intrinsic(
        "molt_importlib_extension_loader_payload", globals()
    )
    _MOLT_IMPORTLIB_SOURCELESS_LOADER_PAYLOAD = _require_intrinsic(
        "molt_importlib_sourceless_loader_payload", globals()
    )
    _MOLT_IMPORTLIB_MODULE_SPEC_IS_PACKAGE = _require_intrinsic(
        "molt_importlib_module_spec_is_package", globals()
    )
    _MOLT_IMPORTLIB_RESOURCES_READER_RESOURCE_PATH_FROM_ROOTS = _require_intrinsic(
        "molt_importlib_resources_reader_resource_path_from_roots", globals()
    )
    _MOLT_IMPORTLIB_RESOURCES_READER_OPEN_RESOURCE_BYTES_FROM_ROOTS = (
        _require_intrinsic(
            "molt_importlib_resources_reader_open_resource_bytes_from_roots", globals()
        )
    )
    _MOLT_IMPORTLIB_RESOURCES_READER_IS_RESOURCE_FROM_ROOTS = _require_intrinsic(
        "molt_importlib_resources_reader_is_resource_from_roots", globals()
    )
    _MOLT_IMPORTLIB_RESOURCES_READER_CONTENTS_FROM_ROOTS = _require_intrinsic(
        "molt_importlib_resources_reader_contents_from_roots", globals()
    )
    _MOLT_IMPORTLIB_PATH_IS_ARCHIVE_MEMBER = _require_intrinsic(
        "molt_importlib_path_is_archive_member", globals()
    )
    _MOLT_IMPORTLIB_PACKAGE_ROOT_FROM_ORIGIN = _require_intrinsic(
        "molt_importlib_package_root_from_origin", globals()
    )
    _MOLT_IMPORTLIB_VALIDATE_RESOURCE_NAME = _require_intrinsic(
        "molt_importlib_validate_resource_name", globals()
    )
    _MOLT_IMPORTLIB_SET_MODULE_STATE = _require_intrinsic(
        "molt_importlib_set_module_state", globals()
    )
    _MOLT_IMPORTLIB_STABILIZE_MODULE_STATE = _require_intrinsic(
        "molt_importlib_stabilize_module_state", globals()
    )
    _MOLT_EXCEPTION_CLEAR = _require_intrinsic("molt_exception_clear", globals())
    _MOLT_MODULE_IMPORT = _require_intrinsic("molt_module_import", globals())


__all__ = [
    "BYTECODE_SUFFIXES",
    "BuiltinImporter",
    "DEBUG_BYTECODE_SUFFIXES",
    "EXTENSION_SUFFIXES",
    "ExtensionFileLoader",
    "FileFinder",
    "FrozenImporter",
    "ModuleSpec",
    "NamespaceLoader",
    "OPTIMIZED_BYTECODE_SUFFIXES",
    "PathFinder",
    "SOURCE_SUFFIXES",
    "SourceFileLoader",
    "SourcelessFileLoader",
    "WindowsRegistryFinder",
    "all_suffixes",
]
