from __future__ import annotations

import builtins
import importlib.util
import os
import sys
from pathlib import Path

import pytest


REPO_ROOT = Path(__file__).resolve().parents[1]
TEMPFILE_MODULE = REPO_ROOT / "src" / "molt" / "stdlib" / "tempfile.py"


def _load_tempfile_module(name: str):
    for key in list(sys.modules):
        if key == name or key.startswith(f"{name}."):
            sys.modules.pop(key, None)

    import tempfile as _real_tempfile

    registry = getattr(builtins, "_molt_intrinsics", None)
    if not isinstance(registry, dict):
        registry = {}
        setattr(builtins, "_molt_intrinsics", registry)
    registry["molt_path_join"] = os.path.join
    registry.setdefault("molt_tempfile_gettempdir", _real_tempfile.gettempdir)
    registry.setdefault("molt_tempfile_gettempdirb", _real_tempfile.gettempdirb)
    registry.setdefault("molt_tempfile_mkdtemp", _real_tempfile.mkdtemp)
    registry.setdefault("molt_tempfile_mkstemp", _real_tempfile.mkstemp)
    registry.setdefault("molt_tempfile_named", lambda *a, **kw: (0, "/tmp/test", True))
    registry.setdefault(
        "molt_tempfile_tempdir", lambda *a, **kw: _real_tempfile.gettempdir()
    )
    registry.setdefault("molt_tempfile_cleanup", lambda path: None)

    def _lookup(intrinsic_name):
        return registry.get(intrinsic_name)

    builtins._molt_intrinsic_lookup = _lookup

    spec = importlib.util.spec_from_file_location(name, TEMPFILE_MODULE)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def test_gettempdir_returns_string() -> None:
    molt_tempfile = _load_tempfile_module("molt_tempfile_gettempdir_test")
    result = molt_tempfile.gettempdir()
    assert isinstance(result, str)
    assert len(result) > 0


def test_gettempdirb_returns_bytes() -> None:
    molt_tempfile = _load_tempfile_module("molt_tempfile_gettempdirb_test")
    result = molt_tempfile.gettempdirb()
    assert isinstance(result, bytes)
    assert len(result) > 0


def test_module_exports_expected_names() -> None:
    molt_tempfile = _load_tempfile_module("molt_tempfile_exports_test")
    assert hasattr(molt_tempfile, "gettempdir")
    assert hasattr(molt_tempfile, "gettempdirb")
    assert hasattr(molt_tempfile, "mkdtemp")
    assert hasattr(molt_tempfile, "mkstemp")
    assert hasattr(molt_tempfile, "NamedTemporaryFile")
    assert hasattr(molt_tempfile, "TemporaryDirectory")


def test_temporary_directory_context_manager() -> None:
    molt_tempfile = _load_tempfile_module("molt_tempfile_tempdir_test")
    td = molt_tempfile.TemporaryDirectory()
    assert isinstance(td.name, str)
    assert len(td.name) > 0
    td.cleanup()
