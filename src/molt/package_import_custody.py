"""Pure package-source admission shared by installed probes and repo tools.

This file is loaded by exact path as ``_molt_package_import_custody``. The
package-neutral name permits admission before importing an ambient ``molt``.
It never changes package bindings, namespace metadata, or search paths.
"""

from __future__ import annotations

import importlib.machinery
import sys
from pathlib import Path
from types import ModuleType
from typing import NoReturn

if __name__ != "_molt_package_import_custody":
    raise ImportError("package import custody requires its selected file loader")


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


def _validate_loaded_package(
    package: str,
    expected_root: Path,
    *,
    reanchor_namespaces: bool,
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
            if reanchor_namespaces:
                updates.append((name, loaded, namespace_root))
            else:
                _require_selected_locations(
                    name, namespace_root, _search_locations(loaded)
                )
            continue
        _require_selected_locations(
            name,
            expected,
            origins + _search_locations(loaded),
        )

    loaded_root = sys.modules.get(package)
    if loaded_root is None:
        if reanchor_namespaces and not (expected / "__init__.py").is_file():
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
    if reanchor_namespaces:
        updates.append((package, loaded_root, expected))
    else:
        _require_selected_locations(package, expected, _search_locations(loaded_root))
    return tuple(updates)


def repository_namespace_updates(
    package: str, expected_root: Path
) -> tuple[tuple[str, ModuleType | None, Path], ...]:
    """Plan tool namespace publication after admitting every executed module.

    The repository binder consumes the complete plan only after every selected
    package validates. Namespace packages have no executed body and can then be
    reanchored; executable modules can never be reanchored.
    """
    return _validate_loaded_package(package, expected_root, reanchor_namespaces=True)


def admit_loaded_package(package: str, expected_root: Path) -> None:
    """Require loaded origins and namespace search paths to be selected.

    Standalone installed probes do not publish or rewrite namespaces. Their
    namespace search locations must already be selected, just like executable
    module origins. This entry point has no deferred publication obligations.
    """
    _validate_loaded_package(package, expected_root, reanchor_namespaces=False)
