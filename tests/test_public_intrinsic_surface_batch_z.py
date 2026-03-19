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

mbc = types.ModuleType("_multibytecodec")
mbc.MultibyteIncrementalEncoder = type("MultibyteIncrementalEncoder", (), {{}})
mbc.MultibyteIncrementalDecoder = type("MultibyteIncrementalDecoder", (), {{}})
mbc.MultibyteStreamReader = type("MultibyteStreamReader", (), {{}})
mbc.MultibyteStreamWriter = type("MultibyteStreamWriter", (), {{}})
sys.modules["_multibytecodec"] = mbc


class _Codec:
    def encode(self, text, errors="strict"):
        return (text.encode("utf-8"), len(text))

    def decode(self, data, errors="strict"):
        return (bytes(data).decode("utf-8", "replace"), len(data))


codecs_jp = types.ModuleType("_codecs_jp")
codecs_jp.getcodec = lambda name: _Codec()
sys.modules["_codecs_jp"] = codecs_jp

cp1125_mod = _load_module("molt_test_cp1125", {str(STDLIB_ROOT / "encodings" / "cp1125.py")!r})
iso8859_8_mod = _load_module("molt_test_iso8859_8", {str(STDLIB_ROOT / "encodings" / "iso8859_8.py")!r})
iso8859_1_mod = _load_module("molt_test_iso8859_1", {str(STDLIB_ROOT / "encodings" / "iso8859_1.py")!r})
mac_roman_mod = _load_module("molt_test_mac_roman", {str(STDLIB_ROOT / "encodings" / "mac_roman.py")!r})
hp_roman8_mod = _load_module("molt_test_hp_roman8", {str(STDLIB_ROOT / "encodings" / "hp_roman8.py")!r})
euc_jis_2004_mod = _load_module("molt_test_euc_jis_2004", {str(STDLIB_ROOT / "encodings" / "euc_jis_2004.py")!r})
cp1140_mod = _load_module("molt_test_cp1140", {str(STDLIB_ROOT / "encodings" / "cp1140.py")!r})
ascii_mod = _load_module("molt_test_ascii", {str(STDLIB_ROOT / "encodings" / "ascii.py")!r})
cp869_mod = _load_module("molt_test_cp869", {str(STDLIB_ROOT / "encodings" / "cp869.py")!r})
cp775_mod = _load_module("molt_test_cp775", {str(STDLIB_ROOT / "encodings" / "cp775.py")!r})

checks = {{
    "cp1125": cp1125_mod.getregentry().name == "cp1125" and "molt_capabilities_has" not in cp1125_mod.__dict__,
    "iso8859_8": iso8859_8_mod.getregentry().name == "iso8859-8" and "molt_capabilities_has" not in iso8859_8_mod.__dict__,
    "iso8859_1": iso8859_1_mod.getregentry().name == "iso8859-1" and "molt_capabilities_has" not in iso8859_1_mod.__dict__,
    "mac_roman": mac_roman_mod.getregentry().name == "mac-roman" and "molt_capabilities_has" not in mac_roman_mod.__dict__,
    "hp_roman8": hp_roman8_mod.getregentry().name == "hp-roman8" and "molt_capabilities_has" not in hp_roman8_mod.__dict__,
    "euc_jis_2004": euc_jis_2004_mod.getregentry().name == "euc_jis_2004" and "molt_capabilities_has" not in euc_jis_2004_mod.__dict__,
    "cp1140": cp1140_mod.getregentry().name == "cp1140" and "molt_capabilities_has" not in cp1140_mod.__dict__,
    "ascii": ascii_mod.getregentry().name == "ascii" and "molt_capabilities_has" not in ascii_mod.__dict__,
    "cp869": cp869_mod.getregentry().name == "cp869" and "molt_capabilities_has" not in cp869_mod.__dict__,
    "cp775": cp775_mod.getregentry().name == "cp775" and "molt_capabilities_has" not in cp775_mod.__dict__,
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_public_intrinsic_surface_batch_z() -> None:
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
        "ascii": "True",
        "cp1125": "True",
        "cp1140": "True",
        "cp775": "True",
        "cp869": "True",
        "euc_jis_2004": "True",
        "hp_roman8": "True",
        "iso8859_1": "True",
        "iso8859_8": "True",
        "mac_roman": "True",
    }
