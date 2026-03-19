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

codecs_jp = types.ModuleType("_codecs_jp")
codecs_jp.getcodec = lambda name: _Codec()
sys.modules["_codecs_jp"] = codecs_jp

codecs_hk = types.ModuleType("_codecs_hk")
codecs_hk.getcodec = lambda name: _Codec()
sys.modules["_codecs_hk"] = codecs_hk

big5hkscs_mod = _load_module("molt_test_big5hkscs", {str(STDLIB_ROOT / "encodings" / "big5hkscs.py")!r})
gb2312_mod = _load_module("molt_test_gb2312", {str(STDLIB_ROOT / "encodings" / "gb2312.py")!r})
euc_jp_mod = _load_module("molt_test_euc_jp", {str(STDLIB_ROOT / "encodings" / "euc_jp.py")!r})
mac_turkish_mod = _load_module("molt_test_mac_turkish", {str(STDLIB_ROOT / "encodings" / "mac_turkish.py")!r})
utf_32_be_mod = _load_module("molt_test_utf_32_be", {str(STDLIB_ROOT / "encodings" / "utf_32_be.py")!r})
mac_croatian_mod = _load_module("molt_test_mac_croatian", {str(STDLIB_ROOT / "encodings" / "mac_croatian.py")!r})
iso8859_7_mod = _load_module("molt_test_iso8859_7", {str(STDLIB_ROOT / "encodings" / "iso8859_7.py")!r})
iso8859_11_mod = _load_module("molt_test_iso8859_11", {str(STDLIB_ROOT / "encodings" / "iso8859_11.py")!r})
cp720_mod = _load_module("molt_test_cp720", {str(STDLIB_ROOT / "encodings" / "cp720.py")!r})
gb18030_mod = _load_module("molt_test_gb18030", {str(STDLIB_ROOT / "encodings" / "gb18030.py")!r})

checks = {{
    "big5hkscs": big5hkscs_mod.getregentry().name == "big5hkscs" and "molt_capabilities_has" not in big5hkscs_mod.__dict__,
    "gb2312": gb2312_mod.getregentry().name == "gb2312" and "molt_capabilities_has" not in gb2312_mod.__dict__,
    "euc_jp": euc_jp_mod.getregentry().name == "euc_jp" and "molt_capabilities_has" not in euc_jp_mod.__dict__,
    "mac_turkish": mac_turkish_mod.getregentry().name == "mac-turkish" and "molt_capabilities_has" not in mac_turkish_mod.__dict__,
    "utf_32_be": utf_32_be_mod.getregentry().name == "utf-32-be" and "molt_capabilities_has" not in utf_32_be_mod.__dict__,
    "mac_croatian": mac_croatian_mod.getregentry().name == "mac-croatian" and "molt_capabilities_has" not in mac_croatian_mod.__dict__,
    "iso8859_7": iso8859_7_mod.getregentry().name == "iso8859-7" and "molt_capabilities_has" not in iso8859_7_mod.__dict__,
    "iso8859_11": iso8859_11_mod.getregentry().name == "iso8859-11" and "molt_capabilities_has" not in iso8859_11_mod.__dict__,
    "cp720": cp720_mod.getregentry().name == "cp720" and "molt_capabilities_has" not in cp720_mod.__dict__,
    "gb18030": gb18030_mod.getregentry().name == "gb18030" and "molt_capabilities_has" not in gb18030_mod.__dict__,
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_public_intrinsic_surface_batch_ac() -> None:
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
        "big5hkscs": "True",
        "cp720": "True",
        "euc_jp": "True",
        "gb18030": "True",
        "gb2312": "True",
        "iso8859_11": "True",
        "iso8859_7": "True",
        "mac_croatian": "True",
        "mac_turkish": "True",
        "utf_32_be": "True",
    }
