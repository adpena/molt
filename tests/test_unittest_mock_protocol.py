"""Host source-boundary proof only; fake loader is not Molt runtime conformance."""

from __future__ import annotations

import ast
import json
from pathlib import Path
import sys

import pytest

from tests.surface_process_guard import run_surface_test_process


ROOT = Path(__file__).resolve().parents[1]
UNITTEST_SOURCE = ROOT / "src/molt/stdlib/unittest"
CORPUS = ROOT / "tests/differential/stdlib/unittest_mock_protocol.py"
RETIRED_FRAGMENTS = ("_mock_autospec", "_mock_patch")

# -I prevents ambient PYTHONPATH from substituting the reference or dependencies.
# Preload dependencies before changing only the canonical unittest package path.
_PROBE = r"""
import asyncio
import contextlib
import copy
import importlib
import inspect
import io
import json
import pickle
import pkgutil
import pprint
import runpy
import sys
import threading
import types
import unittest
import unittest.mock
import unittest.util
from pathlib import Path

mode, source_text, corpus = sys.argv[1:]
source = Path(source_text).resolve()
if mode == "source":
    for name in tuple(sys.modules):
        if name == "unittest.mock" or name.startswith("unittest.mock."):
            del sys.modules[name]
    unittest.__dict__.pop("mock", None)
    unittest.__path__ = [str(source), *unittest.__path__]
    requests = []
    intrinsics = types.ModuleType("_intrinsics")

    def require_intrinsic(name, namespace=None):
        requests.append(name)
        if name != "molt_import_smoke_runtime_ready":
            raise AssertionError("unexpected source-test intrinsic: " + name)
        value = lambda: None
        if namespace is not None:
            namespace[name] = value
        return value

    intrinsics.require_intrinsic = require_intrinsic
    sys.modules["_intrinsics"] = intrinsics
    public = importlib.import_module("unittest.mock")
    assert Path(public.__file__).resolve().parent == source, public.__file__
    assert unittest.mock is public
    original = (public.DEFAULT, public.MagicMock, public.create_autospec, public.patch)
    for repeated in (
        importlib.import_module("unittest.mock"),
        __import__("unittest.mock", fromlist=["mock"]),
    ):
        assert repeated is public
        assert all(
            getattr(repeated, name) is value
            for name, value in zip(
                ("DEFAULT", "MagicMock", "create_autospec", "patch"), original
            )
        )
    for retired in ("unittest._mock_autospec", "unittest._mock_patch"):
        assert retired not in sys.modules
        assert importlib.util.find_spec(retired) is None
    assert requests and set(requests) == {"molt_import_smoke_runtime_ready"}
elif mode == "reference":
    assert Path(unittest.mock.__file__).resolve().parent != source
else:
    raise AssertionError(mode)

runpy.run_path(corpus, run_name="__main__")

if mode == "source":
    assert not any(
        name.startswith("unittest.mock.")
        or name.endswith(("._autospec_context", "._patch_context"))
        for name in sys.modules
    )
    for name, module in tuple(sys.modules.items()):
        if name == "unittest.mock" or name.startswith("unittest._mock"):
            assert "_MOLT_CONTEXT_MODULE" not in vars(module), name
    assert set(requests) == {"molt_import_smoke_runtime_ready"}
"""


def _run_protocol(mode: str) -> dict[str, object]:
    result = run_surface_test_process(
        [
            sys.executable,
            "-I",
            "-c",
            _PROBE,
            mode,
            str(UNITTEST_SOURCE),
            str(CORPUS),
        ],
        cwd=ROOT,
        timeout=45,
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, (
        f"{mode}:\nstdout:\n{result.stdout}\nstderr:\n{result.stderr}"
    )
    return json.loads(result.stdout)


@pytest.fixture(scope="module")
def reference_protocol() -> dict[str, object]:
    return _run_protocol("reference")


def test_mock_protocol_matches_cpython_through_canonical_package_import(
    reference_protocol: dict[str, object],
) -> None:
    assert _run_protocol("source") == reference_protocol


def test_mock_family_has_no_dynamic_fragment_or_context_injection_lane() -> None:
    assert all(
        not (UNITTEST_SOURCE / f"{name}.py").exists() for name in RETIRED_FRAGMENTS
    )
    forbidden_identifiers = {
        "_load_autospec_support",
        "_load_patch_support",
        "_MOLT_CONTEXT_MODULE",
        "spec_from_file_location",
        "module_from_spec",
        "exec_module",
    }
    survivors = []
    # Include any new family authority without prescribing its filename.
    paths = [UNITTEST_SOURCE / "mock.py", *UNITTEST_SOURCE.glob("_mock*.py")]
    for path in paths:
        tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
        for node in ast.walk(tree):
            value = None
            if isinstance(node, ast.Name):
                value = node.id
            elif isinstance(node, ast.Attribute):
                value = node.attr
            elif isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                value = node.name
            elif isinstance(node, ast.Constant) and isinstance(node.value, str):
                value = node.value
            if value in forbidden_identifiers or (
                isinstance(value, str)
                and value.endswith(("._autospec_context", "._patch_context"))
            ):
                survivors.append((path.name, node.lineno, value))
    assert survivors == []
