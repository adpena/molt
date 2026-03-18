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


def _default_value(ctype):
    return 0


def _coerce_value(_ctype, value):
    return int(value)


def _sizeof(obj_or_type):
    size = getattr(obj_or_type, "_size", None)
    if isinstance(size, int):
        return size
    return getattr(type(obj_or_type), "_size", 0)


builtins._molt_intrinsics = {{
    "molt_ctypes_require_ffi": lambda: None,
    "molt_ctypes_coerce_value": _coerce_value,
    "molt_ctypes_default_value": _default_value,
    "molt_ctypes_sizeof": _sizeof,
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


_load_module("ctypes", {str(STDLIB_ROOT / "ctypes" / "__init__.py")!r})
_private = _load_module("_ctypes", {str(STDLIB_ROOT / "_ctypes.py")!r})

rows = [
    (name, type(value).__name__, bool(callable(value)))
    for name, value in sorted(_private.__dict__.items())
    if not name.startswith("_")
]
for name, type_name, is_callable in rows:
    print(f"ROW|{{name}}|{{type_name}}|{{is_callable}}")


class Pair(_private.Structure):
    _fields_ = [("left", _private.c_int), ("right", _private.c_int)]


value = _private.c_int(7)
pair = Pair(3, 4)
ptr = _private.pointer(value)

checks = {{
    "scalar": int(value) == 7 and _private.sizeof(_private.c_int) == 4,
    "structure": pair.left == 3 and pair.right == 4 and _private.sizeof(Pair) == 8,
    "pointer": ptr.contents is value,
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


def test__ctypes_public_surface_matches_expected_shape() -> None:
    rows, checks = _run_probe()
    assert rows == [
        ("Structure", "_StructureMeta", "True"),
        ("c_int", "_CTypeSpec", "True"),
        ("pointer", "function", "True"),
        ("sizeof", "function", "True"),
    ]
    assert checks == {
        "pointer": "True",
        "scalar": "True",
        "structure": "True",
    }
