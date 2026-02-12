from __future__ import annotations

import importlib.util
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "src" / "molt" / "stdlib" / "importlib" / "machinery.py"


def _load_machinery_module():
    spec = importlib.util.spec_from_file_location(
        "molt_stdlib_importlib_machinery", SCRIPT_PATH
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_coerce_module_name_prefers_spec_name_when_module_name_invalid() -> None:
    machinery = _load_machinery_module()

    class _Module:
        __name__ = 123
        __spec__ = type("Spec", (), {"name": "resolved.from.spec"})()

    resolved = machinery._coerce_module_name(_Module(), loader=None)  # noqa: SLF001
    assert resolved == "resolved.from.spec"


def test_coerce_module_name_prefers_loader_name_when_spec_missing() -> None:
    machinery = _load_machinery_module()

    class _Loader:
        name = "resolved.from.loader"

    class _Module:
        __name__ = 123
        __spec__ = None

    resolved = machinery._coerce_module_name(_Module(), loader=_Loader())  # noqa: SLF001
    assert resolved == "resolved.from.loader"


def test_coerce_module_name_raises_without_any_string_source() -> None:
    machinery = _load_machinery_module()

    class _Loader:
        name = 42

    class _Module:
        __name__ = None
        __spec__ = type("Spec", (), {"name": 99})()

    try:
        machinery._coerce_module_name(_Module(), loader=_Loader())  # noqa: SLF001
    except TypeError as exc:
        assert str(exc) == "module name must be str"
    else:
        raise AssertionError("expected TypeError")
