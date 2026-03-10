from __future__ import annotations

import importlib
import importlib.util
import builtins
import sys
from pathlib import Path

import pytest


REPO_ROOT = Path(__file__).resolve().parents[1]
SUPPORT_INIT = (
    REPO_ROOT / "src" / "molt" / "stdlib" / "test" / "support" / "__init__.py"
)


def _load_support_module(name: str):
    for key in list(sys.modules):
        if key == name or key.startswith(f"{name}."):
            sys.modules.pop(key, None)
    spec = importlib.util.spec_from_file_location(
        name,
        SUPPORT_INIT,
        submodule_search_locations=[str(SUPPORT_INIT.parent)],
    )
    assert spec is not None
    assert spec.loader is not None
    registry = getattr(builtins, "_molt_intrinsics", None)
    if not isinstance(registry, dict):
        registry = {}
        setattr(builtins, "_molt_intrinsics", registry)
    registry["molt_capabilities_has"] = lambda _name=None: True
    module = importlib.util.module_from_spec(spec)
    module.__dict__["molt_capabilities_has"] = lambda _name=None: True
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def test_support_facade_prefers_external_cpython_support(
    tmp_path: Path, monkeypatch
) -> None:
    cpython_support = tmp_path / "cpython" / "Lib" / "test" / "support"
    cpython_support.mkdir(parents=True)
    (cpython_support / "__init__.py").write_text(
        "__all__ = ['EXTERNAL_MARKER']\nEXTERNAL_MARKER = 'external-support'\n",
        encoding="utf-8",
    )
    (cpython_support / "import_helper.py").write_text(
        "HELPER_MARKER = 'external-import-helper'\n",
        encoding="utf-8",
    )

    monkeypatch.setenv("MOLT_REGRTEST_CPYTHON_DIR", str(tmp_path / "cpython"))
    module = _load_support_module("molt_test_support_external")

    assert getattr(module, "EXTERNAL_MARKER") == "external-support"
    support_path = Path(getattr(module, "_EXTERNAL_SUPPORT_PATH"))
    assert support_path == (cpython_support / "__init__.py").resolve()
    assert str(cpython_support.resolve()) in list(getattr(module, "__path__"))

    helper = importlib.import_module("molt_test_support_external.import_helper")
    assert getattr(helper, "HELPER_MARKER") == "external-import-helper"


def test_support_facade_uses_local_fallback_without_external(
    tmp_path: Path, monkeypatch
) -> None:
    monkeypatch.delenv("MOLT_REGRTEST_CPYTHON_DIR", raising=False)
    monkeypatch.setattr(sys, "path", [str(tmp_path)])

    module = _load_support_module("molt_test_support_fallback")

    assert hasattr(module, "ALWAYS_EQ")
    assert getattr(module, "_LOADED_EXTERNAL") is False


def test_support_facade_missing_symbol_raises_runtime_error(
    tmp_path: Path, monkeypatch
) -> None:
    monkeypatch.delenv("MOLT_REGRTEST_CPYTHON_DIR", raising=False)
    monkeypatch.setattr(sys, "path", [str(tmp_path)])
    module = _load_support_module("molt_test_support_missing")

    with pytest.raises(RuntimeError, match="MOLT_COMPAT_ERROR"):
        module.__getattr__("missing_symbol")
