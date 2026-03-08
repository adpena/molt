"""CPython-aligned test.support facade for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

import importlib.util
import os
from pathlib import Path
import sys
from types import ModuleType
from typing import Any

_require_intrinsic("molt_capabilities_has", globals())

_THIS_FILE = Path(__file__).resolve()
_THIS_DIR = _THIS_FILE.parent
_EXTERNAL_SUPPORT_MODULE = "_molt_cpython_test_support"
_EXTERNAL_SUPPORT_PATH: str | None = None
_LOADED_EXTERNAL = False


def _candidate_cpython_support_paths() -> list[Path]:
    candidates: list[Path] = []

    cpython_dir = os.environ.get("MOLT_REGRTEST_CPYTHON_DIR", "").strip()
    if cpython_dir:
        candidates.append(
            Path(cpython_dir).expanduser() / "Lib" / "test" / "support" / "__init__.py"
        )

    for entry in sys.path:
        if not entry:
            continue
        candidates.append(Path(entry) / "test" / "support" / "__init__.py")

    out: list[Path] = []
    seen: set[str] = set()
    for candidate in candidates:
        try:
            resolved = candidate.resolve()
        except Exception:
            continue
        if not resolved.exists():
            continue
        if resolved == _THIS_FILE:
            continue
        key = str(resolved)
        if key in seen:
            continue
        seen.add(key)
        out.append(resolved)
    return out


def _load_external_support(path: Path) -> ModuleType | None:
    spec = importlib.util.spec_from_file_location(_EXTERNAL_SUPPORT_MODULE, path)
    if spec is None or spec.loader is None:
        return None
    module = importlib.util.module_from_spec(spec)
    sys.modules[_EXTERNAL_SUPPORT_MODULE] = module
    spec.loader.exec_module(module)
    return module


def _install_external_support(module: ModuleType, support_init: Path) -> None:
    global _EXTERNAL_SUPPORT_PATH
    _EXTERNAL_SUPPORT_PATH = str(support_init)
    external_dir = str(support_init.parent)
    local_dir = str(_THIS_DIR)
    import sys as _ts_sys

    _ts_mod_dict = getattr(_ts_sys.modules.get(__name__), "__dict__", None) or globals()
    _ts_mod_dict["__path__"] = [external_dir, local_dir]

    for name, value in module.__dict__.items():
        if name in {
            "__name__",
            "__loader__",
            "__package__",
            "__spec__",
            "__file__",
            "__cached__",
            "__builtins__",
        }:
            continue
        _ts_mod_dict[name] = value

    all_names = module.__dict__.get("__all__")
    if isinstance(all_names, list):
        _ts_mod_dict["__all__"] = list(all_names)
    elif isinstance(all_names, tuple):
        _ts_mod_dict["__all__"] = list(all_names)


for _candidate in _candidate_cpython_support_paths():
    try:
        _ext = _load_external_support(_candidate)
    except Exception:
        continue
    if _ext is None:
        continue
    _install_external_support(_ext, _candidate)
    _LOADED_EXTERNAL = True
    break

if not _LOADED_EXTERNAL:
    from . import _fallback_support as _fallback

    import sys as _tsf_sys

    _tsf_mod_dict = (
        getattr(_tsf_sys.modules.get(__name__), "__dict__", None) or globals()
    )
    _tsf_mod_dict["__path__"] = [str(_THIS_DIR)]
    _fallback_all = getattr(_fallback, "__all__", [])
    for _name in _fallback_all:
        _tsf_mod_dict[_name] = getattr(_fallback, _name)
    _tsf_mod_dict["__all__"] = list(_fallback_all)


def __dir__() -> list[str]:
    import sys as _tsd_sys

    _tsd_dict = getattr(_tsd_sys.modules.get(__name__), "__dict__", None) or globals()
    all_names = _tsd_dict.get("__all__")
    if isinstance(all_names, list):
        return sorted(set(all_names))
    if isinstance(all_names, tuple):
        return sorted(set(all_names))
    return sorted(name for name in _tsd_dict if not name.startswith("_"))


def __getattr__(name: str) -> Any:
    if name.startswith("__"):
        raise AttributeError(name)
    if _LOADED_EXTERNAL:
        raise AttributeError(name)
    raise RuntimeError(f"MOLT_COMPAT_ERROR: test.support.{name} is not supported")
