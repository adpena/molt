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

cp424_mod = _load_module("molt_test_cp424", {str(STDLIB_ROOT / "encodings" / "cp424.py")!r})
cp1252_mod = _load_module("molt_test_cp1252", {str(STDLIB_ROOT / "encodings" / "cp1252.py")!r})
cp1253_mod = _load_module("molt_test_cp1253", {str(STDLIB_ROOT / "encodings" / "cp1253.py")!r})
iso8859_2_mod = _load_module("molt_test_iso8859_2", {str(STDLIB_ROOT / "encodings" / "iso8859_2.py")!r})
iso8859_3_mod = _load_module("molt_test_iso8859_3", {str(STDLIB_ROOT / "encodings" / "iso8859_3.py")!r})
iso8859_4_mod = _load_module("molt_test_iso8859_4", {str(STDLIB_ROOT / "encodings" / "iso8859_4.py")!r})
iso8859_6_mod = _load_module("molt_test_iso8859_6", {str(STDLIB_ROOT / "encodings" / "iso8859_6.py")!r})
iso8859_10_mod = _load_module("molt_test_iso8859_10", {str(STDLIB_ROOT / "encodings" / "iso8859_10.py")!r})
iso8859_14_mod = _load_module("molt_test_iso8859_14", {str(STDLIB_ROOT / "encodings" / "iso8859_14.py")!r})
iso8859_15_mod = _load_module("molt_test_iso8859_15", {str(STDLIB_ROOT / "encodings" / "iso8859_15.py")!r})

checks = {{
    "cp424": cp424_mod.getregentry().name == "cp424" and "molt_capabilities_has" not in cp424_mod.__dict__,
    "cp1252": cp1252_mod.getregentry().name == "cp1252" and "molt_capabilities_has" not in cp1252_mod.__dict__,
    "cp1253": cp1253_mod.getregentry().name == "cp1253" and "molt_capabilities_has" not in cp1253_mod.__dict__,
    "iso8859_2": iso8859_2_mod.getregentry().name == "iso8859-2" and "molt_capabilities_has" not in iso8859_2_mod.__dict__,
    "iso8859_3": iso8859_3_mod.getregentry().name == "iso8859-3" and "molt_capabilities_has" not in iso8859_3_mod.__dict__,
    "iso8859_4": iso8859_4_mod.getregentry().name == "iso8859-4" and "molt_capabilities_has" not in iso8859_4_mod.__dict__,
    "iso8859_6": iso8859_6_mod.getregentry().name == "iso8859-6" and "molt_capabilities_has" not in iso8859_6_mod.__dict__,
    "iso8859_10": iso8859_10_mod.getregentry().name == "iso8859-10" and "molt_capabilities_has" not in iso8859_10_mod.__dict__,
    "iso8859_14": iso8859_14_mod.getregentry().name == "iso8859-14" and "molt_capabilities_has" not in iso8859_14_mod.__dict__,
    "iso8859_15": iso8859_15_mod.getregentry().name == "iso8859-15" and "molt_capabilities_has" not in iso8859_15_mod.__dict__,
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_public_intrinsic_surface_batch_ae() -> None:
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
        "cp1252": "True",
        "cp1253": "True",
        "cp424": "True",
        "iso8859_10": "True",
        "iso8859_14": "True",
        "iso8859_15": "True",
        "iso8859_2": "True",
        "iso8859_3": "True",
        "iso8859_4": "True",
        "iso8859_6": "True",
    }
