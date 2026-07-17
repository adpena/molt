from __future__ import annotations

import importlib.util
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
