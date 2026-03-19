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


state = {{
    "dispatch": {{}},
    "ext": {{}},
    "inv": {{}},
    "cache": {{}},
    "ctors": set(),
}}

builtins._molt_intrinsics = {{
    "molt_copyreg_bootstrap": lambda: (
        state["dispatch"],
        state["ext"],
        state["inv"],
        state["cache"],
        state["ctors"],
    ),
    "molt_copyreg_pickle": lambda cls, reducer, constructor=None: state["dispatch"].__setitem__(cls, (reducer, constructor)),
    "molt_copyreg_newobj": lambda cls, args: cls(*args),
    "molt_copyreg_newobj_ex": lambda cls, args, kwargs: cls(*args, **kwargs),
    "molt_copyreg_reconstructor": lambda cls, base, state_obj: cls.__new__(cls),
    "molt_copyreg_reduce_ex": lambda obj, proto: ("reduced", proto, type(obj).__name__),
    "molt_copyreg_constructor": lambda func: state["ctors"].add(func),
    "molt_copyreg_add_extension": lambda module, name, code: state["ext"].__setitem__((module, name), int(code)),
    "molt_copyreg_remove_extension": lambda module, name, code: state["ext"].pop((module, name), None),
    "molt_copyreg_clear_extension_cache": lambda: state["cache"].clear(),
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


copyreg = _load_module("copyreg", {str(STDLIB_ROOT / "copyreg.py")!r})

class Payload:
    def __init__(self, value=0):
        self.value = value

def reducer(obj):
    return (Payload, (obj.value,))

copyreg.pickle(Payload, reducer)
copyreg.add_extension("demo.mod", "Payload", 123)
copyreg.remove_extension("demo.mod", "Payload", 123)

checks = {{
    "behavior": (
        Payload in copyreg.dispatch_table
        and copyreg.__newobj__(Payload, 7).value == 7
        and copyreg._reduce_ex(Payload(), 0) == ("reduced", 0, "Payload")
    ),
    "private_handles_hidden": (
        "molt_copyreg_bootstrap" not in copyreg.__dict__
        and "molt_copyreg_pickle" not in copyreg.__dict__
        and "molt_copyreg_newobj" not in copyreg.__dict__
        and "molt_copyreg_add_extension" not in copyreg.__dict__
    ),
}}
for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_copyreg_public_module_hides_raw_intrinsic_names() -> None:
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
