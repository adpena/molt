"""Minimal importlib.machinery support for Molt."""

from __future__ import annotations


class MoltLoader:
    def __repr__(self) -> str:
        return "<MoltLoader>"


MOLT_LOADER = MoltLoader()


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


class SourceFileLoader:
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
            loader=self,
            origin=path,
            is_package=is_package,
            module_package=module_package,
            package_root=package_root,
        )
        _exec_restricted(module, source, path)


class ZipSourceLoader:
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
            loader=self,
            origin=origin,
            is_package=is_package,
            module_package=module_package,
            package_root=package_root,
        )
        _exec_restricted(module, source, origin)


class ExtensionFileLoader:
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


class SourcelessFileLoader:
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
        for path in self._resource_candidates(name):
            payload = _resource_path_payload(path)
            if payload["is_file"] and not payload["is_archive_member"]:
                return path
        raise FileNotFoundError(name)

    def open_resource(self, resource: str):
        name = _validate_resource_name(resource)
        for path in self._resource_candidates(name):
            payload = _resource_path_payload(path)
            if not payload["is_file"]:
                continue
            data = _MOLT_IMPORTLIB_READ_FILE(path)
            if not isinstance(data, bytes):
                raise RuntimeError("invalid importlib read payload: bytes expected")
            import io as _io

            return _io.BytesIO(data)
        raise FileNotFoundError(name)

    def is_resource(self, resource: str) -> bool:
        name = _validate_resource_name(resource)
        for path in self._resource_candidates(name):
            payload = _resource_path_payload(path)
            if payload["is_file"]:
                return True
        return False

    def contents(self) -> list[str]:
        entries: list[str] = []
        for root in self._roots:
            payload = _resource_path_payload(root)
            values = payload["entries"]
            if not isinstance(values, list):
                continue
            for value in values:
                if value not in entries:
                    entries.append(value)
        entries.sort()
        return entries

    def _resource_candidates(self, resource: str) -> list[str]:
        import os as _os

        out: list[str] = []
        for root in self._roots:
            out.append(_os.path.join(root, resource))
        return out


def _set_module_state(
    module,
    *,
    loader: object,
    origin: str,
    is_package: bool,
    module_package: str,
    package_root: str | None,
) -> None:
    spec = getattr(module, "__spec__", None)
    module_name = _coerce_module_name(module, loader, spec)
    if spec is None:
        spec = ModuleSpec(
            module_name,
            loader=loader,
            origin=origin,
            is_package=is_package,
        )
        module.__spec__ = spec
    else:
        if not isinstance(getattr(spec, "name", None), str):
            try:
                spec.name = module_name
            except Exception as exc:
                raise RuntimeError("invalid module spec name state") from exc
        spec.loader = loader
        spec.origin = origin
        spec.has_location = True
    module.__loader__ = loader
    module.__file__ = origin
    module.__cached__ = None
    module.__package__ = module_package
    if is_package:
        if not isinstance(package_root, str):
            raise RuntimeError("invalid importlib package root for package module")
        module.__path__ = [package_root]
        if getattr(spec, "submodule_search_locations", None) is None:
            spec.submodule_search_locations = [package_root]
    import sys as _sys

    _sys.modules[module_name] = module


class PathFinder:
    @classmethod
    def find_spec(
        cls,
        fullname: str,
        path: object | None = None,
        target: object | None = None,
    ):
        del cls
        import importlib.util as _util
        import sys as _sys

        if path is None:
            search_paths = tuple(_sys.path)
            package_context = False
        else:
            search_paths = _util._coerce_search_paths(  # noqa: SLF001
                path,
                "invalid parent package search path",
            )
            package_context = True
        # Provide a non-empty meta-path sentinel so the intrinsic path-hook
        # resolution lane remains active without recursively invoking PathFinder.
        meta_path_sentinel = (None,)
        path_hooks = getattr(_sys, "path_hooks", ())
        path_importer_cache = getattr(_sys, "path_importer_cache", None)
        return _util._find_spec_in_path(  # noqa: SLF001
            fullname,
            list(search_paths),
            meta_path_sentinel,
            path_hooks,
            path_importer_cache,
            package_context,
        )


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


def _normalize_path(path: str) -> str:
    return path.replace("\\", "/")


def _package_root_from_origin(path: str) -> str | None:
    normalized = _normalize_path(path)
    if normalized.endswith("/__init__.py") or normalized.endswith("/__init__.pyc"):
        return normalized.rsplit("/", 1)[0]
    return None


def _validate_resource_name(resource: str) -> str:
    import os as _os

    if not isinstance(resource, str):
        raise TypeError("resource name must be str")
    if not resource or resource in {".", ".."}:
        raise ValueError(f"{resource!r} must be only a file name")
    for sep in ("/", "\\", _os.sep, _os.altsep):
        if sep and sep in resource:
            raise ValueError(f"{resource!r} must be only a file name")
    return resource


def _resource_path_payload(path: str) -> dict[str, object]:
    _ensure_intrinsics()
    payload = _MOLT_IMPORTLIB_RESOURCES_PATH_PAYLOAD(path)
    if not isinstance(payload, dict):
        raise RuntimeError("invalid importlib resources path payload: dict expected")
    exists = payload.get("exists")
    is_file = payload.get("is_file")
    is_dir = payload.get("is_dir")
    entries = payload.get("entries")
    is_archive_member = payload.get("is_archive_member")
    if not isinstance(exists, bool):
        raise RuntimeError("invalid importlib resources path payload: exists")
    if not isinstance(is_file, bool):
        raise RuntimeError("invalid importlib resources path payload: is_file")
    if not isinstance(is_dir, bool):
        raise RuntimeError("invalid importlib resources path payload: is_dir")
    if not isinstance(entries, list) or not all(
        isinstance(entry, str) for entry in entries
    ):
        raise RuntimeError("invalid importlib resources path payload: entries")
    if not isinstance(is_archive_member, bool):
        raise RuntimeError(
            "invalid importlib resources path payload: is_archive_member"
        )
    return {
        "exists": exists,
        "is_file": is_file,
        "is_dir": is_dir,
        "entries": list(entries),
        "is_archive_member": is_archive_member,
    }


def _require_intrinsic(name: str, namespace: dict[str, object] | None = None):
    from _intrinsics import require_intrinsic as _require

    return _require(name, namespace)


def _coerce_module_name(
    module,
    loader: object | None,
    spec: object | None = None,
) -> str:
    module_name = getattr(module, "__name__", None)
    if isinstance(module_name, str):
        return module_name
    module_spec = spec if spec is not None else getattr(module, "__spec__", None)
    spec_name = getattr(module_spec, "name", None)
    if isinstance(spec_name, str):
        _set_module_name_best_effort(module, spec_name)
        return spec_name
    loader_name = getattr(loader, "name", None)
    if isinstance(loader_name, str):
        _set_module_name_best_effort(module, loader_name)
        return loader_name
    raise TypeError("module name must be str")


def _set_module_name_best_effort(module, name: str) -> None:
    try:
        module.__name__ = name
    except Exception:
        return


_MOLT_IMPORTLIB_SOURCE_EXEC_PAYLOAD = None
_MOLT_IMPORTLIB_ZIP_SOURCE_EXEC_PAYLOAD = None
_MOLT_IMPORTLIB_READ_FILE = None
_MOLT_IMPORTLIB_RESOURCES_PATH_PAYLOAD = None
_MOLT_IMPORTLIB_EXEC_RESTRICTED_SOURCE = None
_MOLT_IMPORTLIB_EXEC_EXTENSION = None
_MOLT_IMPORTLIB_EXEC_SOURCELESS = None
_MOLT_IMPORTLIB_EXTENSION_LOADER_PAYLOAD = None
_MOLT_IMPORTLIB_SOURCELESS_LOADER_PAYLOAD = None
_MOLT_IMPORTLIB_MODULE_SPEC_IS_PACKAGE = None
_MOLT_EXCEPTION_CLEAR = None


def _ensure_intrinsics() -> None:
    global _MOLT_IMPORTLIB_SOURCE_EXEC_PAYLOAD
    global _MOLT_IMPORTLIB_ZIP_SOURCE_EXEC_PAYLOAD
    global _MOLT_IMPORTLIB_READ_FILE
    global _MOLT_IMPORTLIB_RESOURCES_PATH_PAYLOAD
    global _MOLT_IMPORTLIB_EXEC_RESTRICTED_SOURCE
    global _MOLT_IMPORTLIB_EXEC_EXTENSION
    global _MOLT_IMPORTLIB_EXEC_SOURCELESS
    global _MOLT_IMPORTLIB_EXTENSION_LOADER_PAYLOAD
    global _MOLT_IMPORTLIB_SOURCELESS_LOADER_PAYLOAD
    global _MOLT_IMPORTLIB_MODULE_SPEC_IS_PACKAGE
    global _MOLT_EXCEPTION_CLEAR
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
    _MOLT_IMPORTLIB_RESOURCES_PATH_PAYLOAD = _require_intrinsic(
        "molt_importlib_resources_path_payload", globals()
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
    _MOLT_EXCEPTION_CLEAR = _require_intrinsic("molt_exception_clear", globals())


__all__ = [
    "ExtensionFileLoader",
    "ModuleSpec",
    "MOLT_LOADER",
    "MoltLoader",
    "PathFinder",
    "SourcelessFileLoader",
    "SourceFileLoader",
    "ZipSourceLoader",
    "_exec_restricted",
]
