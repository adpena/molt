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
    "molt_capabilities_has": lambda _name=None: True,
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

_abc_mod = types.ModuleType("_abc")
_abc_mod.get_cache_token = lambda: 17
sys.modules["_abc"] = _abc_mod

_weakrefset_mod = types.ModuleType("_weakrefset")
class WeakSet:
    pass
_weakrefset_mod.WeakSet = WeakSet
sys.modules["_weakrefset"] = _weakrefset_mod

abc_mod = types.ModuleType("abc")
class ABCMeta(type):
    pass
abc_mod.ABCMeta = ABCMeta
sys.modules["abc"] = abc_mod


def _load_module(name, path_text):
    spec = importlib.util.spec_from_file_location(name, path_text)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


_private = _load_module("molt_test__py_abc", {str(STDLIB_ROOT / "_py_abc.py")!r})

checks = {{
    "anchor": "molt_capabilities_has" not in _private.__dict__,
    "behavior": (
        _private.get_cache_token() == 17
        and _private.ABCMeta is ABCMeta
        and _private.WeakSet is WeakSet
    ),
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def _run_probe() -> dict[str, str]:
    proc = subprocess.run(
        [sys.executable, "-c", _PROBE],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=True,
    )
    checks: dict[str, str] = {}
    for line in proc.stdout.splitlines():
        prefix, key, value = line.split("|", 2)
        assert prefix == "CHECK"
        checks[key] = value
    return checks


def test_py_abc_private_module_surface() -> None:
    assert _run_probe() == {"anchor": "True", "behavior": "True"}
