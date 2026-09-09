from __future__ import annotations

import importlib.machinery
import importlib.util
import sys
from dataclasses import dataclass
from pathlib import Path
from types import ModuleType
from typing import NoReturn

_CANONICAL_MODULE_NAME = "tools.import_file"
_current_module = sys.modules[__name__]
_canonical_module = sys.modules.get(_CANONICAL_MODULE_NAME)
_reuse_canonical = False

# A file-launched tool can initially see this file only as top-level
# ``import_file``. If the canonical module is already present, make this import
# return that exact object and do not create a second set of implementations.
# Otherwise publish the file-launched object canonically before its body loads.
if __name__ == "import_file":
    if _canonical_module is not None and _canonical_module is not _current_module:
        canonical_file = getattr(_canonical_module, "__file__", None)
        if not isinstance(canonical_file, str) or (
            Path(canonical_file).resolve() != Path(__file__).resolve()
        ):
            raise RuntimeError(
                "repository import authority is already loaded from another location"
            )
        sys.modules[__name__] = _canonical_module
        _reuse_canonical = True
    else:
        sys.modules[_CANONICAL_MODULE_NAME] = _current_module
        loaded_tools = sys.modules.get("tools")
        if loaded_tools is not None:
            setattr(loaded_tools, "import_file", _current_module)


if not _reuse_canonical:

    @dataclass(frozen=True, slots=True)
    class _MissingModuleBinding:
        pass

    _MISSING = _MissingModuleBinding()

    def _repository_root(source_file: str | Path) -> Path:
        source = Path(source_file).resolve()
        for candidate in (source.parent, *source.parents):
            if (candidate / "pyproject.toml").is_file() and (
                candidate / "tools"
            ).is_dir():
                return candidate
        raise RuntimeError(f"cannot locate Molt repository root from {source}")

    def _executable_origins(module: ModuleType) -> tuple[object, ...]:
        origins: list[object] = []
        module_file = getattr(module, "__file__", None)
        if module_file is not None:
            origins.append(module_file)
        spec = getattr(module, "__spec__", None)
        spec_origin = getattr(spec, "origin", None)
        if spec_origin is not None:
            origins.append(spec_origin)
        return tuple(origins)

    def _search_locations(module: ModuleType) -> tuple[object, ...]:
        locations: list[object] = list(getattr(module, "__path__", ()))
        spec = getattr(module, "__spec__", None)
        spec_locations = getattr(spec, "submodule_search_locations", None)
        if spec_locations is not None:
            locations.extend(spec_locations)
        return tuple(locations)

    def _custody_mismatch(
        name: str,
        expected: Path,
        values: tuple[object, ...],
    ) -> NoReturn:
        detail = ", ".join(str(value) for value in values) or "unknown"
        raise RuntimeError(
            f"repository import custody mismatch for {name}: expected {expected}, "
            f"loaded {detail}"
        )

    def _require_selected_locations(
        name: str,
        expected: Path,
        values: tuple[object, ...],
    ) -> None:
        if not values:
            _custody_mismatch(name, expected, values)
        for value in values:
            if not isinstance(value, str) or value in {"built-in", "frozen"}:
                _custody_mismatch(name, expected, values)
            location = Path(value).resolve()
            if not location.is_relative_to(expected):
                _custody_mismatch(name, expected, values)

    def _is_namespace_package(module: ModuleType) -> bool:
        spec = getattr(module, "__spec__", None)
        if spec is None or getattr(module, "__path__", None) is None:
            return False
        if getattr(spec, "submodule_search_locations", None) is None:
            return False
        loader = getattr(spec, "loader", None)
        return loader is None or isinstance(loader, importlib.machinery.NamespaceLoader)

    def _canonical_namespace_prototype(package: str, expected: Path) -> ModuleType:
        search_locations = [str(expected)]

        def find_selected_namespace(
            fullname: str,
            _parent_path: object,
        ) -> importlib.machinery.ModuleSpec:
            selected = importlib.machinery.PathFinder.find_spec(
                fullname,
                [str(expected.parent)],
            )
            if selected is None:
                raise ImportError(f"selected namespace source disappeared: {expected}")
            return selected

        loader = importlib.machinery.NamespaceLoader(
            package,
            search_locations,
            find_selected_namespace,
        )
        spec = importlib.machinery.ModuleSpec(package, loader, is_package=True)
        spec.submodule_search_locations = search_locations
        return importlib.util.module_from_spec(spec)

    def _validate_loaded_package(
        package: str,
        expected_root: Path,
    ) -> tuple[tuple[str, ModuleType | None, Path], ...]:
        expected = expected_root.resolve()
        updates: list[tuple[str, ModuleType | None, Path]] = []
        for name, loaded in tuple(sys.modules.items()):
            if not name.startswith(f"{package}."):
                continue
            if loaded is None:
                continue
            origins = _executable_origins(loaded)
            if not origins:
                if not _is_namespace_package(loaded):
                    _custody_mismatch(name, expected, origins)
                suffix = name.removeprefix(f"{package}.").split(".")
                namespace_root = expected.joinpath(*suffix)
                if (
                    not namespace_root.is_dir()
                    or (namespace_root / "__init__.py").is_file()
                ):
                    _custody_mismatch(name, namespace_root, _search_locations(loaded))
                updates.append(
                    (
                        name,
                        loaded,
                        namespace_root,
                    )
                )
                continue
            _require_selected_locations(
                name,
                expected,
                origins + _search_locations(loaded),
            )

        loaded_root = sys.modules.get(package)
        if loaded_root is None:
            if not (expected / "__init__.py").is_file():
                updates.append((package, None, expected))
            return tuple(updates)
        origins = _executable_origins(loaded_root)
        if origins:
            _require_selected_locations(
                package,
                expected,
                origins + _search_locations(loaded_root),
            )
            return tuple(updates)
        if not _is_namespace_package(loaded_root):
            _custody_mismatch(package, expected, _search_locations(loaded_root))
        if (expected / "__init__.py").is_file():
            _custody_mismatch(package, expected, _search_locations(loaded_root))
        updates.append(
            (
                package,
                loaded_root,
                expected,
            )
        )
        return tuple(updates)

    def _install_namespace_metadata(
        loaded: ModuleType,
        prototype: ModuleType,
    ) -> None:
        loaded.__package__ = prototype.__package__
        loaded.__loader__ = prototype.__loader__
        loaded.__spec__ = prototype.__spec__
        loaded.__path__ = prototype.__path__

    def _publish_namespace(package: str, prototype: ModuleType) -> None:
        sys.modules[package] = prototype
        prefix = f"{package}."
        for name, loaded in tuple(sys.modules.items()):
            child = name.removeprefix(prefix)
            if name.startswith(prefix) and "." not in child and loaded is not None:
                setattr(prototype, child, loaded)

    def bind_repository_imports(source_file: str | Path) -> Path:
        """Bind file-launched imports without reanchoring loaded packages."""

        root = _repository_root(source_file)
        source_root = root / "src"
        namespace_updates = (
            *_validate_loaded_package("tools", root / "tools"),
            *_validate_loaded_package("molt", source_root / "molt"),
        )
        # NamespaceLoader observes its parent package when constructed. Validate
        # the whole family first, then restore parents before nested namespaces.
        for name, loaded, expected in sorted(
            namespace_updates, key=lambda update: (update[0].count("."), update[0])
        ):
            prototype = _canonical_namespace_prototype(name, expected)
            if loaded is None:
                _publish_namespace(name, prototype)
            else:
                _install_namespace_metadata(loaded, prototype)
        selected_roots = (root.resolve(), source_root.resolve())
        sys.path[:] = [
            item
            for item in sys.path
            if not (
                isinstance(item, str)
                and any(Path(item).resolve() == selected for selected in selected_roots)
            )
        ]
        for import_root in (root, source_root):
            sys.path.insert(0, str(import_root))
        return root

    def load_module_from_path(module_name: str, path: Path) -> ModuleType:
        """Execute *path* with normal import transaction semantics.

        ``module_from_spec`` does not register the module. Register it before
        execution because dataclasses, pickling, and recursive imports consult
        ``sys.modules`` while the body runs. A failed body restores the exact
        prior binding.
        """

        spec = importlib.util.spec_from_file_location(module_name, path)
        if spec is None or spec.loader is None:
            raise ImportError(f"cannot load module {module_name!r} from {path}")
        module = importlib.util.module_from_spec(spec)
        previous = sys.modules.get(module_name, _MISSING)
        sys.modules[module_name] = module
        try:
            spec.loader.exec_module(module)
        except BaseException:
            if isinstance(previous, _MissingModuleBinding):
                sys.modules.pop(module_name, None)
            else:
                sys.modules[module_name] = previous
            raise
        return module

    def load_sibling_package_module_from_path(
        module_name: str,
        path: Path,
    ) -> ModuleType:
        """Load one file inside a transactional synthetic sibling package.

        This gives relative imports normal package semantics without adding an
        ambient directory to ``sys.path``. Parent and descendant bindings are
        restored together when the module body fails.
        """

        package_name, separator, _child_name = module_name.rpartition(".")
        if not separator or not package_name:
            raise ValueError(
                "sibling package module name must include a parent package"
            )
        package_path = path.resolve().parent
        previous_package = sys.modules.get(package_name, _MISSING)
        sibling_prefix = f"{package_name}."
        previous_siblings = {
            name: module
            for name, module in sys.modules.items()
            if name.startswith(sibling_prefix)
        }
        package = ModuleType(package_name)
        package.__package__ = package_name
        setattr(package, "__path__", [str(package_path)])
        package_spec = importlib.machinery.ModuleSpec(
            package_name,
            loader=None,
            is_package=True,
        )
        package_spec.submodule_search_locations = [str(package_path)]
        package.__spec__ = package_spec
        sys.modules[package_name] = package
        try:
            return load_module_from_path(module_name, path)
        except BaseException:
            for name in tuple(sys.modules):
                if name.startswith(sibling_prefix) and name not in previous_siblings:
                    sys.modules.pop(name, None)
            sys.modules.update(previous_siblings)
            if isinstance(previous_package, _MissingModuleBinding):
                sys.modules.pop(package_name, None)
            else:
                sys.modules[package_name] = previous_package
            raise
