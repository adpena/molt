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


_drops = []
_handles = [0]


def _molt_semaphore_new(value):
    _handles[0] += 1
    return ("sem", _handles[0], value)


def _molt_semaphore_drop(handle):
    _drops.append(handle)


def _molt_process_drop(name):
    return ("unlink", name)


builtins._molt_intrinsics = {{
    "molt_capabilities_has": lambda _name=None: True,
    "molt_process_drop": _molt_process_drop,
    "molt_semaphore_new": _molt_semaphore_new,
    "molt_semaphore_drop": _molt_semaphore_drop,
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


_private = _load_module("molt_test__multiprocessing", {str(STDLIB_ROOT / "_multiprocessing.py")!r})
s = _private.SemLock(3)
handle = s._handle
del s

checks = {{
    "anchor": "molt_capabilities_has" not in _private.__dict__,
    "private_intrinsics": (
        "molt_process_drop" not in _private.__dict__
        and "molt_semaphore_new" not in _private.__dict__
        and "molt_semaphore_drop" not in _private.__dict__
    ),
    "behavior": (
        _private.sem_unlink("x") == ("unlink", "x")
        and handle in _drops
        and _private.flags == {{}}
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


def test_multiprocessing_private_module_surface() -> None:
    assert _run_probe() == {
        "anchor": "True",
        "behavior": "True",
        "private_intrinsics": "True",
    }
