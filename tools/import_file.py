from __future__ import annotations

import importlib.machinery
import importlib.util
import sys
from dataclasses import dataclass
from pathlib import Path
from types import ModuleType

_CANONICAL_MODULE_NAME = "tools.import_file"
_current_module = sys.modules[__name__]
_canonical_module = sys.modules.get(_CANONICAL_MODULE_NAME)
_reuse_canonical = False

# A file-launched tool can initially see this file only as top-level
# ``import_file``. If the canonical module is already present, make this import
# return that exact object and do not create a second set of implementations.
# A new file-launched object is published canonically only after its body loads.
# Python already registers its requested name for dataclasses and recursion.
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


if not _reuse_canonical:

    def _load_package_import_custody(path: Path) -> ModuleType:
        """Load one neutral authority; never import it through a foreign parent."""
        name = "_molt_package_import_custody"
        selected = path.resolve(strict=True)
        if name in sys.modules:
            loaded = sys.modules[name]
            spec = getattr(loaded, "__spec__", None)
            loader = getattr(spec, "loader", None)
            if (
                type(loaded) is not ModuleType
                or getattr(loaded, "__name__", None) != name
                or getattr(loaded, "__package__", None) != ""
                or getattr(loaded, "__file__", None) != str(selected)
                or getattr(spec, "name", None) != name
                or getattr(spec, "origin", None) != str(selected)
                or type(loader) is not importlib.machinery.SourceFileLoader
                or loader.name != name
                or loader.path != str(selected)
                or getattr(loaded, "__loader__", None) is not loader
            ):
                raise ImportError(
                    f"package import custody already loaded from another authority; "
                    f"selected {selected}, loaded {getattr(loaded, '__file__', None)!r}"
                )
            return loaded
        spec = importlib.util.spec_from_file_location(name, selected)
        if spec is None or spec.loader is None:
            raise ImportError(
                f"cannot load selected package import custody: {selected}"
            )
        loaded = importlib.util.module_from_spec(spec)
        sys.modules[name] = loaded
        try:
            spec.loader.exec_module(loaded)
        except BaseException:
            sys.modules.pop(name, None)
            raise
        return loaded

    _package_import_custody = _load_package_import_custody(
        Path(__file__).resolve().parents[1] / "src/molt/package_import_custody.py"
    )

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
            *_package_import_custody.repository_namespace_updates(
                "tools", root / "tools"
            ),
            *_package_import_custody.repository_namespace_updates(
                "molt", source_root / "molt"
            ),
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


if __name__ == "import_file" and not _reuse_canonical:
    sys.modules[_CANONICAL_MODULE_NAME] = _current_module
    loaded_tools = sys.modules.get("tools")
    if loaded_tools is not None:
        setattr(loaded_tools, "import_file", _current_module)
