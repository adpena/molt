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

cp1251_mod = _load_module("molt_test_cp1251", {str(STDLIB_ROOT / "encodings" / "cp1251.py")!r})
cp1006_mod = _load_module("molt_test_cp1006", {str(STDLIB_ROOT / "encodings" / "cp1006.py")!r})
cp037_mod = _load_module("molt_test_cp037", {str(STDLIB_ROOT / "encodings" / "cp037.py")!r})
tis620_mod = _load_module("molt_test_tis_620", {str(STDLIB_ROOT / "encodings" / "tis_620.py")!r})
iso8859_16_mod = _load_module("molt_test_iso8859_16", {str(STDLIB_ROOT / "encodings" / "iso8859_16.py")!r})
cp1250_mod = _load_module("molt_test_cp1250", {str(STDLIB_ROOT / "encodings" / "cp1250.py")!r})
cp500_mod = _load_module("molt_test_cp500", {str(STDLIB_ROOT / "encodings" / "cp500.py")!r})
cp1256_mod = _load_module("molt_test_cp1256", {str(STDLIB_ROOT / "encodings" / "cp1256.py")!r})
cp1257_mod = _load_module("molt_test_cp1257", {str(STDLIB_ROOT / "encodings" / "cp1257.py")!r})
cp874_mod = _load_module("molt_test_cp874", {str(STDLIB_ROOT / "encodings" / "cp874.py")!r})

checks = {{
    "cp1251": cp1251_mod.getregentry().name == "cp1251" and "molt_capabilities_has" not in cp1251_mod.__dict__,
    "cp1006": cp1006_mod.getregentry().name == "cp1006" and "molt_capabilities_has" not in cp1006_mod.__dict__,
    "cp037": cp037_mod.getregentry().name == "cp037" and "molt_capabilities_has" not in cp037_mod.__dict__,
    "tis_620": tis620_mod.getregentry().name == "tis-620" and "molt_capabilities_has" not in tis620_mod.__dict__,
    "iso8859_16": iso8859_16_mod.getregentry().name == "iso8859-16" and "molt_capabilities_has" not in iso8859_16_mod.__dict__,
    "cp1250": cp1250_mod.getregentry().name == "cp1250" and "molt_capabilities_has" not in cp1250_mod.__dict__,
    "cp500": cp500_mod.getregentry().name == "cp500" and "molt_capabilities_has" not in cp500_mod.__dict__,
    "cp1256": cp1256_mod.getregentry().name == "cp1256" and "molt_capabilities_has" not in cp1256_mod.__dict__,
    "cp1257": cp1257_mod.getregentry().name == "cp1257" and "molt_capabilities_has" not in cp1257_mod.__dict__,
    "cp874": cp874_mod.getregentry().name == "cp874" and "molt_capabilities_has" not in cp874_mod.__dict__,
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_public_intrinsic_surface_batch_aa() -> None:
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
        "cp037": "True",
        "cp1006": "True",
        "cp1250": "True",
        "cp1251": "True",
        "cp1256": "True",
        "cp1257": "True",
        "cp500": "True",
        "cp874": "True",
        "iso8859_16": "True",
        "tis_620": "True",
    }
