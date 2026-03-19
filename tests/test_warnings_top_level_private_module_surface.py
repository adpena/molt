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


def _warn(message, category=None, stacklevel=1, source=None):
    return ("warn", str(message), category.__name__ if category else None, int(stacklevel))


def _warn_explicit(message, category, filename, lineno, module=None, registry=None, module_globals=None, source=None):
    return ("warn_explicit", str(message), getattr(category, "__name__", None), str(filename), int(lineno))


builtins._molt_intrinsics = {{
    "molt_stdlib_probe": lambda: True,
    "molt_capabilities_has": lambda _name=None: True,
    "molt_warnings_warn": _warn,
    "molt_warnings_warn_explicit": _warn_explicit,
    "molt_warnings_formatwarning": lambda *args: "formatted",
    "molt_warnings_showwarning": lambda *args: None,
    "molt_warnings_simplefilter": lambda *args: None,
    "molt_warnings_filterwarnings": lambda *args: None,
    "molt_warnings_resetwarnings": lambda *args: None,
    "molt_warnings_filters_get": lambda: [("default", None, Warning, None, 0)],
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


_load_module("warnings", {str(STDLIB_ROOT / "warnings.py")!r})
_private = _load_module("_warnings", {str(STDLIB_ROOT / "_warnings.py")!r})

rows = [
    (name, type(value).__name__, bool(callable(value)))
    for name, value in sorted(_private.__dict__.items())
    if not name.startswith("_")
]
for name, type_name, is_callable in rows:
    print(f"ROW|{{name}}|{{type_name}}|{{is_callable}}")

checks = {{
    "filters": isinstance(_private.filters, list),
    "warn": _private.warn("hello", UserWarning, 2) is None,
    "warn_explicit": _private.warn_explicit("boom", UserWarning, "file.py", 7) is None,
    "warnings_module_private_handles_hidden": (
        "_MOLT_STDLIB_PROBE" not in sys.modules["warnings"].__dict__
        and "_MOLT_CAPABILITIES_HAS" not in sys.modules["warnings"].__dict__
        and "molt_stdlib_probe" not in sys.modules["warnings"].__dict__
        and "molt_capabilities_has" not in sys.modules["warnings"].__dict__
        and "molt_warnings_warn" not in sys.modules["warnings"].__dict__
        and "molt_warnings_warn_explicit" not in sys.modules["warnings"].__dict__
    ),
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


def test__warnings_public_surface_matches_expected_shape() -> None:
    rows, checks = _run_probe()
    assert rows == [
        ("filters", "list", "False"),
        ("warn", "function", "True"),
        ("warn_explicit", "function", "True"),
    ]
    assert checks == {
        "filters": "True",
        "warn": "True",
        "warn_explicit": "True",
        "warnings_module_private_handles_hidden": "True",
    }
