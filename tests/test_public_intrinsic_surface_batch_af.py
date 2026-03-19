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
    "molt_quopri_encode": lambda data, quotetabs=False, header=False: bytes(data),
    "molt_quopri_decode": lambda data, header=False: bytes(data),
    "molt_uu_codec_encode": lambda data, filename=None, mode=None: bytes(data),
    "molt_uu_codec_decode": lambda data: bytes(data),
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

quopri_mod = _load_module("molt_test_quopri_codec", {str(STDLIB_ROOT / "encodings" / "quopri_codec.py")!r})
bz2_mod = _load_module("molt_test_bz2_codec", {str(STDLIB_ROOT / "encodings" / "bz2_codec.py")!r})
hex_mod = _load_module("molt_test_hex_codec", {str(STDLIB_ROOT / "encodings" / "hex_codec.py")!r})
base64_mod = _load_module("molt_test_base64_codec", {str(STDLIB_ROOT / "encodings" / "base64_codec.py")!r})
uu_mod = _load_module("molt_test_uu_codec", {str(STDLIB_ROOT / "encodings" / "uu_codec.py")!r})
utf_16_le_mod = _load_module("molt_test_utf_16_le", {str(STDLIB_ROOT / "encodings" / "utf_16_le.py")!r})
utf_16_mod = _load_module("molt_test_utf_16", {str(STDLIB_ROOT / "encodings" / "utf_16.py")!r})
utf_8_mod = _load_module("molt_test_utf_8", {str(STDLIB_ROOT / "encodings" / "utf_8.py")!r})
utf_7_mod = _load_module("molt_test_utf_7", {str(STDLIB_ROOT / "encodings" / "utf_7.py")!r})
utf_32_le_mod = _load_module("molt_test_utf_32_le", {str(STDLIB_ROOT / "encodings" / "utf_32_le.py")!r})

checks = {{
    "quopri": quopri_mod.getregentry().name == "quopri"
    and "molt_quopri_encode" not in quopri_mod.__dict__
    and "molt_quopri_decode" not in quopri_mod.__dict__,
    "bz2": bz2_mod.getregentry().name == "bz2" and "molt_capabilities_has" not in bz2_mod.__dict__,
    "hex": hex_mod.getregentry().name == "hex" and "molt_capabilities_has" not in hex_mod.__dict__,
    "base64": base64_mod.getregentry().name == "base64" and "molt_capabilities_has" not in base64_mod.__dict__,
    "uu": uu_mod.getregentry().name == "uu"
    and "molt_capabilities_has" not in uu_mod.__dict__
    and "molt_uu_codec_encode" not in uu_mod.__dict__
    and "molt_uu_codec_decode" not in uu_mod.__dict__,
    "utf_16_le": utf_16_le_mod.getregentry().name == "utf-16-le" and "molt_capabilities_has" not in utf_16_le_mod.__dict__,
    "utf_16": utf_16_mod.getregentry().name == "utf-16" and "molt_capabilities_has" not in utf_16_mod.__dict__,
    "utf_8": utf_8_mod.getregentry().name == "utf-8" and "molt_capabilities_has" not in utf_8_mod.__dict__,
    "utf_7": utf_7_mod.getregentry().name == "utf-7" and "molt_capabilities_has" not in utf_7_mod.__dict__,
    "utf_32_le": utf_32_le_mod.getregentry().name == "utf-32-le" and "molt_capabilities_has" not in utf_32_le_mod.__dict__,
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_public_intrinsic_surface_batch_af() -> None:
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
        "base64": "True",
        "bz2": "True",
        "hex": "True",
        "quopri": "True",
        "utf_16": "True",
        "utf_16_le": "True",
        "utf_32_le": "True",
        "utf_7": "True",
        "utf_8": "True",
        "uu": "True",
    }
