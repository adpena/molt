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


codecs_cn = types.ModuleType("_codecs_cn")
codecs_cn.getcodec = lambda name: _Codec()
sys.modules["_codecs_cn"] = codecs_cn

koi8_t_mod = _load_module("molt_test_koi8_t", {str(STDLIB_ROOT / "encodings" / "koi8_t.py")!r})
utf_8_sig_mod = _load_module("molt_test_utf_8_sig", {str(STDLIB_ROOT / "encodings" / "utf_8_sig.py")!r})
cp864_mod = _load_module("molt_test_cp864", {str(STDLIB_ROOT / "encodings" / "cp864.py")!r})
kz1048_mod = _load_module("molt_test_kz1048", {str(STDLIB_ROOT / "encodings" / "kz1048.py")!r})
mac_cyrillic_mod = _load_module("molt_test_mac_cyrillic", {str(STDLIB_ROOT / "encodings" / "mac_cyrillic.py")!r})
cp866_mod = _load_module("molt_test_cp866", {str(STDLIB_ROOT / "encodings" / "cp866.py")!r})
cp1258_mod = _load_module("molt_test_cp1258", {str(STDLIB_ROOT / "encodings" / "cp1258.py")!r})
hz_mod = _load_module("molt_test_hz", {str(STDLIB_ROOT / "encodings" / "hz.py")!r})
utf_16_be_mod = _load_module("molt_test_utf_16_be", {str(STDLIB_ROOT / "encodings" / "utf_16_be.py")!r})
cp862_mod = _load_module("molt_test_cp862", {str(STDLIB_ROOT / "encodings" / "cp862.py")!r})

checks = {{
    "koi8_t": koi8_t_mod.getregentry().name == "koi8-t" and "molt_capabilities_has" not in koi8_t_mod.__dict__,
    "utf_8_sig": utf_8_sig_mod.getregentry().name == "utf-8-sig" and "molt_capabilities_has" not in utf_8_sig_mod.__dict__,
    "cp864": cp864_mod.getregentry().name == "cp864" and "molt_capabilities_has" not in cp864_mod.__dict__,
    "kz1048": kz1048_mod.getregentry().name == "kz1048" and "molt_capabilities_has" not in kz1048_mod.__dict__,
    "mac_cyrillic": mac_cyrillic_mod.getregentry().name == "mac-cyrillic" and "molt_capabilities_has" not in mac_cyrillic_mod.__dict__,
    "cp866": cp866_mod.getregentry().name == "cp866" and "molt_capabilities_has" not in cp866_mod.__dict__,
    "cp1258": cp1258_mod.getregentry().name == "cp1258" and "molt_capabilities_has" not in cp1258_mod.__dict__,
    "hz": hz_mod.getregentry().name == "hz" and "molt_capabilities_has" not in hz_mod.__dict__,
    "utf_16_be": utf_16_be_mod.getregentry().name == "utf-16-be" and "molt_capabilities_has" not in utf_16_be_mod.__dict__,
    "cp862": cp862_mod.getregentry().name == "cp862" and "molt_capabilities_has" not in cp862_mod.__dict__,
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_public_intrinsic_surface_batch_y() -> None:
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
        "cp1258": "True",
        "cp862": "True",
        "cp864": "True",
        "cp866": "True",
        "hz": "True",
        "koi8_t": "True",
        "kz1048": "True",
        "mac_cyrillic": "True",
        "utf_16_be": "True",
        "utf_8_sig": "True",
    }
