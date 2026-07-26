from __future__ import annotations

import importlib.util
import importlib.machinery
import sys
from pathlib import Path
from types import ModuleType

_MISSING = object()


def load_module_from_path(module_name: str, path: Path) -> ModuleType:
    """Execute *path* with normal import transaction semantics.

    ``module_from_spec`` does not register the module. Loaders that execute a
    file directly must do so before ``exec_module``: dataclasses, pickling,
    recursive imports, and other module-identity consumers consult
    ``sys.modules`` while the body runs. A failed execution restores the prior
    binding so a broken worktree module cannot poison the host process.
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
        if previous is _MISSING:
            sys.modules.pop(module_name, None)
        else:
            sys.modules[module_name] = previous  # type: ignore[assignment]
        raise
    return module


def load_sibling_package_module_from_path(
    module_name: str,
    path: Path,
) -> ModuleType:
    """Load one file inside a synthetic package rooted at its directory.

    Repository tools sometimes need a module from ``src`` before that package is
    installed or importable. Loading the file as a top-level module breaks its
    relative sibling imports; mutating ``sys.path`` instead makes the result
    depend on ambient package precedence. A synthetic package gives the module
    normal relative-import semantics while keeping resolution pinned to the
    exact sibling directory selected by the caller.

    ``module_name`` must contain its synthetic parent package, for example
    ``_molt_authority_0123.process_guard``. Both parent and child bindings are
    transactional: a failed body restores whatever the host process had before.
    """

    package_name, separator, _child_name = module_name.rpartition(".")
    if not separator or not package_name:
        raise ValueError("sibling package module name must include a parent package")
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
    package.__path__ = [str(package_path)]  # type: ignore[attr-defined]
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
        if previous_package is _MISSING:
            sys.modules.pop(package_name, None)
        else:
            sys.modules[package_name] = previous_package  # type: ignore[assignment]
        raise
