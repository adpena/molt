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


def _load_module(name, path_text):
    spec = importlib.util.spec_from_file_location(name, path_text)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


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

mp_pkg = types.ModuleType("multiprocessing")
mp_pkg.__path__ = [{str(STDLIB_ROOT / "multiprocessing")!r}]
sys.modules["multiprocessing"] = mp_pkg

api_surface_mod = types.ModuleType("multiprocessing._api_surface")


def apply_module_api_surface(name, namespace):
    namespace["APPLIED"] = name


api_surface_mod.apply_module_api_surface = apply_module_api_surface
sys.modules["multiprocessing._api_surface"] = api_surface_mod

textpad_mod = _load_module("molt_test_curses_textpad", {str(STDLIB_ROOT / "curses" / "textpad.py")!r})
wintypes_mod = _load_module("molt_test_ctypes_wintypes", {str(STDLIB_ROOT / "ctypes" / "wintypes.py")!r})
simple_interact_mod = _load_module("molt_test_simple_interact", {str(STDLIB_ROOT / "_pyrepl" / "simple_interact.py")!r})
resource_sharer_mod = _load_module("multiprocessing.resource_sharer", {str(STDLIB_ROOT / "multiprocessing" / "resource_sharer.py")!r})
managers_mod = _load_module("multiprocessing.managers", {str(STDLIB_ROOT / "multiprocessing" / "managers.py")!r})


def _raises_runtimeerror(mod, attr):
    try:
        getattr(mod, attr)
    except RuntimeError:
        return True
    return False


checks = {{
    "textpad": (
        textpad_mod.Textbox(object()).edit() == ""
        and "molt_capabilities_has" not in textpad_mod.__dict__
    ),
    "wintypes": (
        hasattr(wintypes_mod, "DWORD")
        and "molt_capabilities_has" not in wintypes_mod.__dict__
    ),
    "simple_interact": (
        _raises_runtimeerror(simple_interact_mod, "run_multiline_interactive_console")
        and "molt_capabilities_has" not in simple_interact_mod.__dict__
    ),
    "resource_sharer": (
        resource_sharer_mod.APPLIED == "multiprocessing.resource_sharer"
        and "molt_capabilities_has" not in resource_sharer_mod.__dict__
    ),
    "managers": (
        managers_mod.APPLIED == "multiprocessing.managers"
        and "molt_capabilities_has" not in managers_mod.__dict__
    ),
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_public_intrinsic_surface_batch_t() -> None:
    proc = subprocess.run(
        [sys.executable, "-c", _PROBE],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=True,
    )
    checks: dict[str, str] = {}
    for line in proc.stdout.splitlines():
        if line.startswith("CHECK|"):
            _, key, value = line.split("|", 2)
            checks[key] = value
    assert checks == {
        "managers": "True",
        "resource_sharer": "True",
        "simple_interact": "True",
        "textpad": "True",
        "wintypes": "True",
    }
