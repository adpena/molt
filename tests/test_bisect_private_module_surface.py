from __future__ import annotations

import bisect as _host_bisect
import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"

_PROBE = f"""
import builtins
import importlib.util
import bisect as _host_bisect
import sys
import types


builtins._molt_intrinsics = {{
    "molt_bisect_left": _host_bisect.bisect_left,
    "molt_bisect_right": _host_bisect.bisect_right,
    "molt_bisect_insort_left": _host_bisect.insort_left,
    "molt_bisect_insort_right": _host_bisect.insort_right,
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


_private = _load_module("_bisect", {str(STDLIB_ROOT / "_bisect.py")!r})

rows = [
    (name, type(getattr(_private, name)).__name__, bool(callable(getattr(_private, name))))
    for name in sorted(dir(_private))
    if not name.startswith("_")
]
for name, type_name, is_callable in rows:
    print(f"ROW|{{name}}|{{type_name}}|{{is_callable}}")

data = [1, 3, 5]
left = _private.bisect_left(data, 3)
right = _private.bisect_right(data, 3)
_private.insort_left(data, 4)
checks = {{
    "behavior": left == 1 and right == 2 and data == [1, 3, 4, 5],
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


def test__bisect_public_surface_matches_expected_shape() -> None:
    rows, checks = _run_probe()
    assert rows == [
        ("bisect_left", "builtin_function_or_method", "True"),
        ("bisect_right", "builtin_function_or_method", "True"),
        ("insort_left", "builtin_function_or_method", "True"),
        ("insort_right", "builtin_function_or_method", "True"),
    ]
    assert checks == {"behavior": "True"}
