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

    def _loaded_locations(module: ModuleType) -> tuple[Path, ...]:
        locations: list[Path] = []
        module_file = getattr(module, "__file__", None)
        if isinstance(module_file, str):
            locations.append(Path(module_file).resolve())
        module_path = getattr(module, "__path__", ())
        locations.extend(Path(item).resolve() for item in module_path)
        return tuple(locations)

    def _require_loaded_package_root(package: str, expected_root: Path) -> None:
        expected = expected_root.resolve()
        for name, loaded in tuple(sys.modules.items()):
            if name != package and not name.startswith(f"{package}."):
                continue
            if loaded is None:
                continue
            locations = _loaded_locations(loaded)
            if not locations or any(
                not location.is_relative_to(expected) for location in locations
            ):
                detail = ", ".join(str(location) for location in locations) or "unknown"
                raise RuntimeError(
                    f"repository import custody mismatch for {name}: expected {expected}, "
                    f"loaded {detail}"
                )

    def bind_repository_imports(source_file: str | Path) -> Path:
        """Bind file-launched imports without reanchoring loaded packages."""

        root = _repository_root(source_file)
        source_root = root / "src"
        _require_loaded_package_root("tools", root / "tools")
        _require_loaded_package_root("molt", source_root / "molt")
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
