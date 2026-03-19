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


builtins._molt_intrinsics = {{
    "molt_abc_get_cache_token": lambda: 1,
    "molt_abc_init": lambda cls: None,
    "molt_abc_register": lambda cls, subcls: subcls,
    "molt_abc_instancecheck": lambda cls, inst: False,
    "molt_abc_subclasscheck": lambda cls, subcls: False,
    "molt_abc_get_dump": lambda cls: (),
    "molt_abc_reset_registry": lambda cls: None,
    "molt_abc_reset_caches": lambda cls: None,
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


_private = _load_module("_molt_private_abc", {str(STDLIB_ROOT / "_abc.py")!r})

rows = [
    (name, type(getattr(_private, name)).__name__, bool(callable(getattr(_private, name))))
    for name in sorted(dir(_private))
    if not name.startswith("__") and name != "annotations"
]
for name, type_name, is_callable in rows:
    print(f"ROW|{{name}}|{{type_name}}|{{is_callable}}")

checks = {{
    "behavior": (
        _private.get_cache_token() == 1
        and _private._abc_register(object, int) is int
        and _private._abc_instancecheck(object, 1) is False
        and _private._abc_subclasscheck(object, int) is False
        and _private._get_dump(object) == ()
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


def test__abc_public_surface_matches_expected_shape() -> None:
    rows, checks = _run_probe()
    assert rows == [
        ("_abc_init", "function", "True"),
        ("_abc_instancecheck", "function", "True"),
        ("_abc_register", "function", "True"),
        ("_abc_subclasscheck", "function", "True"),
        ("_get_dump", "function", "True"),
        ("_reset_caches", "function", "True"),
        ("_reset_registry", "function", "True"),
        ("get_cache_token", "function", "True"),
    ]
    assert checks == {"behavior": "True"}
