"""Minimal importlib.machinery support for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


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
        payload = _MOLT_IMPORTLIB_READ_FILE(path)
        if not isinstance(payload, bytes):
            raise RuntimeError("invalid importlib read payload: bytes expected")
        return payload

    def create_module(self, _spec: ModuleSpec):
        return None

    def exec_module(self, module) -> None:
        import sys

        path = self.path
        spec = getattr(module, "__spec__", None)
        spec_has_locations = (
            spec is not None
            and getattr(spec, "submodule_search_locations", None) is not None
        )
        payload = _source_exec_payload(module.__name__, path, bool(spec_has_locations))
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
        if spec is None:
            spec = ModuleSpec(
                module.__name__,
                loader=self,
                origin=path,
                is_package=is_package,
            )
            module.__spec__ = spec
        if getattr(module, "__loader__", None) is None:
            module.__loader__ = self
        module.__file__ = path
        module.__cached__ = None
        module.__package__ = module_package
        if is_package:
            if not isinstance(package_root, str):
                raise RuntimeError(
                    "invalid importlib source loader payload: package_root"
                )
            module.__path__ = [package_root]
            if spec.submodule_search_locations is None:
                spec.submodule_search_locations = [package_root]
        sys.modules[module.__name__] = module
        result = _MOLT_IMPORTLIB_EXEC_RESTRICTED_SOURCE(module.__dict__, source, path)
        if result is not None:
            raise RuntimeError(
                "invalid importlib source execution intrinsic result: expected None"
            )
        cleared = _MOLT_EXCEPTION_CLEAR()
        if cleared is not None:
            raise RuntimeError(
                "invalid exception clear intrinsic result: expected None"
            )


def _source_exec_payload(
    module_name: str, path: str, spec_is_package: bool
) -> dict[str, object]:
    payload = _MOLT_IMPORTLIB_SOURCE_EXEC_PAYLOAD(module_name, path, spec_is_package)
    if not isinstance(payload, dict):
        raise RuntimeError("invalid importlib source exec payload: dict expected")
    return payload


_require_intrinsic("molt_stdlib_probe", globals())
_MOLT_IMPORTLIB_SOURCE_EXEC_PAYLOAD = _require_intrinsic(
    "molt_importlib_source_exec_payload", globals()
)
_MOLT_IMPORTLIB_READ_FILE = _require_intrinsic("molt_importlib_read_file", globals())
_MOLT_IMPORTLIB_EXEC_RESTRICTED_SOURCE = _require_intrinsic(
    "molt_importlib_exec_restricted_source", globals()
)
_MOLT_EXCEPTION_CLEAR = _require_intrinsic("molt_exception_clear", globals())


__all__ = ["ModuleSpec", "MOLT_LOADER", "MoltLoader", "SourceFileLoader"]
