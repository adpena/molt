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


class ReferenceType:
    pass


class ProxyType:
    pass


class CallableProxyType:
    pass


def ref(obj):
    return ("ref", obj)


def proxy(obj):
    return ("proxy", obj)


def getweakrefcount(obj):
    return 7


def getweakrefs(obj):
    return [("ref", obj)]


_fake_weakref = types.ModuleType("weakref")
_fake_weakref.ReferenceType = ReferenceType
_fake_weakref.ProxyType = ProxyType
_fake_weakref.CallableProxyType = CallableProxyType
_fake_weakref.ref = ref
_fake_weakref.proxy = proxy
_fake_weakref.getweakrefcount = getweakrefcount
_fake_weakref.getweakrefs = getweakrefs
sys.modules["weakref"] = _fake_weakref

builtins._molt_intrinsics = {{
    "molt_weakref_count": lambda obj: 7,
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


_private = _load_module("_weakref", {str(STDLIB_ROOT / "_weakref.py")!r})

rows = [
    (name, type(getattr(_private, name)).__name__, bool(callable(getattr(_private, name))))
    for name in sorted(dir(_private))
    if not name.startswith("_") and name != "annotations"
]
for name, type_name, is_callable in rows:
    print(f"ROW|{{name}}|{{type_name}}|{{is_callable}}")

checks = {{
    "behavior": (
        _private.ref("x") == ("ref", "x")
        and _private.proxy("x") == ("proxy", "x")
        and _private.getweakrefcount("x") == 7
        and _private.getweakrefs("x") == [("ref", "x")]
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


def test__weakref_public_surface_matches_expected_shape() -> None:
    rows, checks = _run_probe()
    assert rows == [
        ("CallableProxyType", "type", "True"),
        ("ProxyType", "type", "True"),
        ("ReferenceType", "type", "True"),
        ("getweakrefcount", "function", "True"),
        ("getweakrefs", "function", "True"),
        ("proxy", "function", "True"),
        ("ref", "function", "True"),
    ]
    assert checks == {"behavior": "True"}
