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

cp273_mod = _load_module("molt_test_cp273", {str(STDLIB_ROOT / "encodings" / "cp273.py")!r})
cp856_mod = _load_module("molt_test_cp856", {str(STDLIB_ROOT / "encodings" / "cp856.py")!r})
cp875_mod = _load_module("molt_test_cp875", {str(STDLIB_ROOT / "encodings" / "cp875.py")!r})
latin_1_mod = _load_module("molt_test_latin_1", {str(STDLIB_ROOT / "encodings" / "latin_1.py")!r})
mac_arabic_mod = _load_module("molt_test_mac_arabic", {str(STDLIB_ROOT / "encodings" / "mac_arabic.py")!r})
mac_farsi_mod = _load_module("molt_test_mac_farsi", {str(STDLIB_ROOT / "encodings" / "mac_farsi.py")!r})
mac_greek_mod = _load_module("molt_test_mac_greek", {str(STDLIB_ROOT / "encodings" / "mac_greek.py")!r})
mac_iceland_mod = _load_module("molt_test_mac_iceland", {str(STDLIB_ROOT / "encodings" / "mac_iceland.py")!r})
mac_latin2_mod = _load_module("molt_test_mac_latin2", {str(STDLIB_ROOT / "encodings" / "mac_latin2.py")!r})
mac_romanian_mod = _load_module("molt_test_mac_romanian", {str(STDLIB_ROOT / "encodings" / "mac_romanian.py")!r})

checks = {{
    "cp273": cp273_mod.getregentry().name == "cp273" and "molt_capabilities_has" not in cp273_mod.__dict__,
    "cp856": cp856_mod.getregentry().name == "cp856" and "molt_capabilities_has" not in cp856_mod.__dict__,
    "cp875": cp875_mod.getregentry().name == "cp875" and "molt_capabilities_has" not in cp875_mod.__dict__,
    "latin_1": latin_1_mod.getregentry().name == "iso8859-1" and "molt_capabilities_has" not in latin_1_mod.__dict__,
    "mac_arabic": mac_arabic_mod.getregentry().name == "mac-arabic" and "molt_capabilities_has" not in mac_arabic_mod.__dict__,
    "mac_farsi": mac_farsi_mod.getregentry().name == "mac-farsi" and "molt_capabilities_has" not in mac_farsi_mod.__dict__,
    "mac_greek": mac_greek_mod.getregentry().name == "mac-greek" and "molt_capabilities_has" not in mac_greek_mod.__dict__,
    "mac_iceland": mac_iceland_mod.getregentry().name == "mac-iceland" and "molt_capabilities_has" not in mac_iceland_mod.__dict__,
    "mac_latin2": mac_latin2_mod.getregentry().name == "mac-latin2" and "molt_capabilities_has" not in mac_latin2_mod.__dict__,
    "mac_romanian": mac_romanian_mod.getregentry().name == "mac-romanian" and "molt_capabilities_has" not in mac_romanian_mod.__dict__,
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_public_intrinsic_surface_batch_ag() -> None:
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
        "cp273": "True",
        "cp856": "True",
        "cp875": "True",
        "latin_1": "True",
        "mac_arabic": "True",
        "mac_farsi": "True",
        "mac_greek": "True",
        "mac_iceland": "True",
        "mac_latin2": "True",
        "mac_romanian": "True",
    }
