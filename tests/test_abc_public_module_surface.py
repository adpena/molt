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


calls = []

builtins._molt_intrinsics = {{
    "molt_abc_get_cache_token": lambda: 5,
    "molt_abc_init": lambda cls: calls.append(("init", cls.__name__)),
    "molt_abc_register": lambda cls, subcls: subcls,
    "molt_abc_instancecheck": lambda cls, inst: False,
    "molt_abc_subclasscheck": lambda cls, subcls: False,
    "molt_abc_get_dump": lambda cls: (),
    "molt_abc_reset_registry": lambda cls: None,
    "molt_abc_reset_caches": lambda cls: None,
    "molt_abc_bootstrap": lambda: None,
    "molt_abc_update_abstractmethods": lambda cls: cls,
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


_load_module("_abc", {str(STDLIB_ROOT / "_abc.py")!r})
abc = _load_module("abc", {str(STDLIB_ROOT / "abc.py")!r})

class Base(abc.ABC):
    @abc.abstractmethod
    def f(self):
        raise NotImplementedError

class Impl(Base):
    def f(self):
        return 1

updated = abc.update_abstractmethods(Impl)

checks = {{
    "behavior": (
        updated is Impl
        and abc.get_cache_token() == 5
        and Impl().f() == 1
        and ("init", "ABC") in calls
        and ("init", "Base") in calls
        and ("init", "Impl") in calls
    ),
    "private_handles_hidden": (
        "_MOLT_ABC_BOOTSTRAP" not in abc.__dict__
        and "_MOLT_ABC_UPDATE_ABSTRACTMETHODS" not in abc.__dict__
        and "molt_abc_bootstrap" not in abc.__dict__
        and "molt_abc_update_abstractmethods" not in abc.__dict__
    ),
}}
for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_abc_public_module_hides_bootstrap_handles() -> None:
    proc = subprocess.run(
        [sys.executable, "-c", _PROBE],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=True,
    )
    checks: dict[str, str] = {}
    for line in proc.stdout.splitlines():
        prefix, *rest = line.split("|")
        if prefix == "CHECK":
            checks[rest[0]] = rest[1]
    assert checks == {"behavior": "True", "private_handles_hidden": "True"}
