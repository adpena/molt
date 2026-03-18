from __future__ import annotations

import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"

_PROBE = f"""
import builtins
import importlib.util
import sys
import types
import warnings as _host_warnings

builtins._molt_intrinsics = {{
    "molt_stdlib_probe": lambda: True,
    "molt_capabilities_has": lambda _name=None: True,
    "molt_warnings_warn": _host_warnings.warn,
    "molt_warnings_warn_explicit": _host_warnings.warn_explicit,
    "molt_warnings_formatwarning": _host_warnings.formatwarning,
    "molt_warnings_showwarning": _host_warnings.showwarning,
    "molt_warnings_simplefilter": _host_warnings.simplefilter,
    "molt_warnings_filterwarnings": _host_warnings.filterwarnings,
    "molt_warnings_resetwarnings": _host_warnings.resetwarnings,
    "molt_warnings_filters_get": lambda: list(_host_warnings.filters),
}}

_intrinsics_mod = types.ModuleType("_intrinsics")


def _require_intrinsic(name, namespace=None):
    intrinsics = getattr(builtins, "_molt_intrinsics", {{}})
    if name in intrinsics:
        value = intrinsics[name]
        if namespace is not None:
            namespace[name] = value
        return value
    raise RuntimeError(f"intrinsic unavailable: {{name}}")


_intrinsics_mod.require_intrinsic = _require_intrinsic
sys.modules["_intrinsics"] = _intrinsics_mod


def _load_module(name, path_text):
    spec = importlib.util.spec_from_file_location(name, path_text)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


_warnings_mod = _load_module("warnings", {str(STDLIB_ROOT / "warnings.py")!r})
_py_warnings = _load_module("_py_warnings", {str(STDLIB_ROOT / "_py_warnings.py")!r})

rows = [
    (name, type(value).__name__, bool(callable(value)))
    for name, value in sorted(_py_warnings.__dict__.items())
    if not name.startswith("_")
]

for name, type_name, is_callable in rows:
    print(f"ROW|{{name}}|{{type_name}}|{{is_callable}}")

checks = {{
    "aliases": (
        _py_warnings.catch_warnings is _warnings_mod._CatchWarnings
        and _py_warnings.warn is _warnings_mod.warn
        and _py_warnings.warn_explicit is _warnings_mod.warn_explicit
        and _py_warnings.WarningMessage is _warnings_mod._WarningRecord
        and _py_warnings.defaultaction == _warnings_mod._default_action
        and _py_warnings.filters is _warnings_mod._filters
    ),
    "onceregistry_shape": isinstance(_py_warnings.onceregistry, dict),
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def _run_probe() -> tuple[list[tuple[str, str, str]], dict[str, str]]:
    proc = subprocess.run(
        [sys.executable, "-c", _PROBE],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=True,
    )
    rows: list[tuple[str, str, str]] = []
    checks: dict[str, str] = {}
    for line in proc.stdout.splitlines():
        prefix, *rest = line.split("|")
        if prefix == "ROW":
            rows.append((rest[0], rest[1], rest[2]))
        elif prefix == "CHECK":
            checks[rest[0]] = rest[1]
    return rows, checks


def test__py_warnings_public_surface_matches_expected_shape() -> None:
    rows, checks = _run_probe()
    assert rows == [
        ("WarningMessage", "type", "True"),
        ("catch_warnings", "type", "True"),
        ("defaultaction", "str", "False"),
        ("deprecated", "type", "True"),
        ("filters", "list", "False"),
        ("filterwarnings", "function", "True"),
        ("formatwarning", "function", "True"),
        ("onceregistry", "dict", "False"),
        ("resetwarnings", "function", "True"),
        ("showwarning", "function", "True"),
        ("simplefilter", "function", "True"),
        ("sys", "module", "False"),
        ("warn", "function", "True"),
        ("warn_explicit", "function", "True"),
    ]
    assert checks == {"aliases": "True", "onceregistry_shape": "True"}
