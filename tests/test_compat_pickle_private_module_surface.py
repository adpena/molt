from __future__ import annotations

import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"

_PROBE = f"""
import importlib.util
import sys
import types


_intrinsics_mod = types.ModuleType("_intrinsics")


def _require_intrinsic(name, namespace=None):
    if name != "molt_capabilities_has":
        raise RuntimeError(f"intrinsic unavailable: {{name}}")
    value = lambda _name=None: True
    if namespace is not None:
        namespace[name] = value
    return value


_intrinsics_mod.require_intrinsic = _require_intrinsic
sys.modules["_intrinsics"] = _intrinsics_mod


def _load_module(name, path_text):
    spec = importlib.util.spec_from_file_location(name, path_text)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


mod = _load_module("_compat_pickle", {str(STDLIB_ROOT / "_compat_pickle.py")!r})

rows = [
    (name, type(value).__name__, bool(callable(value)))
    for name, value in sorted(mod.__dict__.items())
    if not name.startswith("_")
]
for name, type_name, is_callable in rows:
    print(f"ROW|{{name}}|{{type_name}}|{{is_callable}}")

checks = {{
    "imports": (
        mod.IMPORT_MAPPING["Tkinter"] == "tkinter"
        and mod.REVERSE_IMPORT_MAPPING["tkinter"] == "Tkinter"
        and mod.IMPORT_MAPPING["cStringIO"] == "io"
    ),
    "names": (
        mod.NAME_MAPPING[("__builtin__", "xrange")] == ("builtins", "range")
        and mod.REVERSE_NAME_MAPPING[("builtins", "range")] == ("__builtin__", "xrange")
        and mod.NAME_MAPPING[("urllib2", "HTTPError")] == ("urllib.error", "HTTPError")
    ),
    "exceptions": (
        "ModuleNotFoundError" in mod.PYTHON3_IMPORTERROR_EXCEPTIONS
        and "TimeoutError" in mod.PYTHON3_OSERROR_EXCEPTIONS
        and "ProcessError" in mod.MULTIPROCESSING_EXCEPTIONS
        and "UnicodeWarning" in mod.PYTHON2_EXCEPTIONS
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


def test__compat_pickle_public_surface_matches_expected_shape() -> None:
    rows, checks = _run_probe()
    assert rows == [
        ("IMPORT_MAPPING", "dict", "False"),
        ("MULTIPROCESSING_EXCEPTIONS", "tuple", "False"),
        ("NAME_MAPPING", "dict", "False"),
        ("PYTHON2_EXCEPTIONS", "tuple", "False"),
        ("PYTHON3_IMPORTERROR_EXCEPTIONS", "tuple", "False"),
        ("PYTHON3_OSERROR_EXCEPTIONS", "tuple", "False"),
        ("REVERSE_IMPORT_MAPPING", "dict", "False"),
        ("REVERSE_NAME_MAPPING", "dict", "False"),
    ]
    assert checks == {
        "exceptions": "True",
        "imports": "True",
        "names": "True",
    }
