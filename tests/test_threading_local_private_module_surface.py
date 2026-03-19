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

_fake_contextlib = types.ModuleType("contextlib")
_fake_contextlib.contextmanager = lambda fn: fn
sys.modules["contextlib"] = _fake_contextlib

_fake_threading = types.ModuleType("threading")
class _RLock:
    pass
_fake_threading.RLock = _RLock
_fake_threading.current_thread = lambda: "thread"
class _local:
    pass
_fake_threading.local = _local
sys.modules["threading"] = _fake_threading

_fake_weakref = types.ModuleType("weakref")
class ReferenceType:
    pass
_fake_weakref.ReferenceType = ReferenceType
sys.modules["weakref"] = _fake_weakref

builtins._molt_intrinsics = {{
    "molt_capabilities_has": lambda name: True,
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


_private = _load_module("_molt_private_threading_local", {str(STDLIB_ROOT / "_threading_local.py")!r})

rows = [
    (name, type(getattr(_private, name)).__name__, bool(callable(getattr(_private, name))))
    for name in sorted(dir(_private))
    if not name.startswith("_") and name != "annotations"
]
for name, type_name, is_callable in rows:
    print(f"ROW|{{name}}|{{type_name}}|{{is_callable}}")

checks = {{
    "behavior": (
        _private.current_thread() == "thread"
        and _private.RLock().__class__.__name__ == "_RLock"
        and _private.local.__name__ == "_local"
        and _private.ref is ReferenceType
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


def test__threading_local_public_surface_matches_expected_shape() -> None:
    rows, checks = _run_probe()
    assert rows == [
        ("RLock", "function", "True"),
        ("contextmanager", "function", "True"),
        ("current_thread", "function", "True"),
        ("local", "type", "True"),
        ("ref", "type", "True"),
    ]
    assert checks == {"behavior": "True"}
