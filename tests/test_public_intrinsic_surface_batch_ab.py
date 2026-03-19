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

cp850_mod = _load_module("molt_test_cp850", {str(STDLIB_ROOT / "encodings" / "cp850.py")!r})
cp855_mod = _load_module("molt_test_cp855", {str(STDLIB_ROOT / "encodings" / "cp855.py")!r})
cp858_mod = _load_module("molt_test_cp858", {str(STDLIB_ROOT / "encodings" / "cp858.py")!r})
cp861_mod = _load_module("molt_test_cp861", {str(STDLIB_ROOT / "encodings" / "cp861.py")!r})
cp857_mod = _load_module("molt_test_cp857", {str(STDLIB_ROOT / "encodings" / "cp857.py")!r})
cp860_mod = _load_module("molt_test_cp860", {str(STDLIB_ROOT / "encodings" / "cp860.py")!r})
cp863_mod = _load_module("molt_test_cp863", {str(STDLIB_ROOT / "encodings" / "cp863.py")!r})
cp852_mod = _load_module("molt_test_cp852", {str(STDLIB_ROOT / "encodings" / "cp852.py")!r})
cp737_mod = _load_module("molt_test_cp737", {str(STDLIB_ROOT / "encodings" / "cp737.py")!r})
cp865_mod = _load_module("molt_test_cp865", {str(STDLIB_ROOT / "encodings" / "cp865.py")!r})

checks = {{
    "cp850": cp850_mod.getregentry().name == "cp850" and "molt_capabilities_has" not in cp850_mod.__dict__,
    "cp855": cp855_mod.getregentry().name == "cp855" and "molt_capabilities_has" not in cp855_mod.__dict__,
    "cp858": cp858_mod.getregentry().name == "cp858" and "molt_capabilities_has" not in cp858_mod.__dict__,
    "cp861": cp861_mod.getregentry().name == "cp861" and "molt_capabilities_has" not in cp861_mod.__dict__,
    "cp857": cp857_mod.getregentry().name == "cp857" and "molt_capabilities_has" not in cp857_mod.__dict__,
    "cp860": cp860_mod.getregentry().name == "cp860" and "molt_capabilities_has" not in cp860_mod.__dict__,
    "cp863": cp863_mod.getregentry().name == "cp863" and "molt_capabilities_has" not in cp863_mod.__dict__,
    "cp852": cp852_mod.getregentry().name == "cp852" and "molt_capabilities_has" not in cp852_mod.__dict__,
    "cp737": cp737_mod.getregentry().name == "cp737" and "molt_capabilities_has" not in cp737_mod.__dict__,
    "cp865": cp865_mod.getregentry().name == "cp865" and "molt_capabilities_has" not in cp865_mod.__dict__,
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_public_intrinsic_surface_batch_ab() -> None:
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
        "cp737": "True",
        "cp850": "True",
        "cp852": "True",
        "cp855": "True",
        "cp857": "True",
        "cp858": "True",
        "cp860": "True",
        "cp861": "True",
        "cp863": "True",
        "cp865": "True",
    }
