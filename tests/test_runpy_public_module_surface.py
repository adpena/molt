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
    "molt_runpy_run_module": lambda mod_name, run_name, init_globals, alter_sys: {{
        "kind": "module",
        "mod_name": mod_name,
        "run_name": run_name,
        "alter_sys": alter_sys,
        "seed": None if init_globals is None else init_globals.get("seed"),
    }},
    "molt_runpy_resolve_path": lambda path, module_file: {{
        "abspath": f"/abs/{{path}}",
        "is_file": True,
    }},
    "molt_runpy_run_path": lambda path, run_name, init_globals: {{
        "kind": "path",
        "path": path,
        "run_name": run_name,
        "seed": None if init_globals is None else init_globals.get("seed"),
    }},
    "molt_capabilities_trusted": lambda: False,
    "molt_capabilities_require": lambda cap: calls.append(cap),
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


runpy = _load_module("runpy", {str(STDLIB_ROOT / "runpy.py")!r})
module_ns = runpy.run_module("demo.mod", init_globals={{"seed": 7}}, run_name="demo", alter_sys=True)
path_ns = runpy.run_path("script.py", init_globals={{"seed": 9}}, run_name="__main__")

checks = {{
    "behavior": (
        module_ns == {{"kind": "module", "mod_name": "demo.mod", "run_name": "demo", "alter_sys": True, "seed": 7}}
        and path_ns == {{"kind": "path", "path": "/abs/script.py", "run_name": "__main__", "seed": 9}}
        and calls == ["fs.read"]
    ),
    "private_handles_hidden": (
        "_molt_runpy_run_module" not in runpy.__dict__
        and "_molt_runpy_resolve_path" not in runpy.__dict__
        and "_molt_runpy_run_path" not in runpy.__dict__
        and "_molt_capabilities_trusted" not in runpy.__dict__
        and "_molt_capabilities_require" not in runpy.__dict__
        and "molt_runpy_run_module" not in runpy.__dict__
        and "molt_runpy_resolve_path" not in runpy.__dict__
        and "molt_runpy_run_path" not in runpy.__dict__
    ),
}}
for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_runpy_public_module_hides_intrinsic_handles() -> None:
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
