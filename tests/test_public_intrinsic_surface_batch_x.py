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


codecs_kr = types.ModuleType("_codecs_kr")
codecs_kr.getcodec = lambda name: _Codec()
sys.modules["_codecs_kr"] = codecs_kr

codecs_cn = types.ModuleType("_codecs_cn")
codecs_cn.getcodec = lambda name: _Codec()
sys.modules["_codecs_cn"] = codecs_cn

johab_mod = _load_module("molt_test_johab", {str(STDLIB_ROOT / "encodings" / "johab.py")!r})
iso8859_9_mod = _load_module("molt_test_iso8859_9", {str(STDLIB_ROOT / "encodings" / "iso8859_9.py")!r})
cp1254_mod = _load_module("molt_test_cp1254", {str(STDLIB_ROOT / "encodings" / "cp1254.py")!r})
gbk_mod = _load_module("molt_test_gbk", {str(STDLIB_ROOT / "encodings" / "gbk.py")!r})
palmos_mod = _load_module("molt_test_palmos", {str(STDLIB_ROOT / "encodings" / "palmos.py")!r})
koi8_u_mod = _load_module("molt_test_koi8_u", {str(STDLIB_ROOT / "encodings" / "koi8_u.py")!r})
zlib_codec_mod = _load_module("molt_test_zlib_codec", {str(STDLIB_ROOT / "encodings" / "zlib_codec.py")!r})
utf_32_mod = _load_module("molt_test_utf_32", {str(STDLIB_ROOT / "encodings" / "utf_32.py")!r})
cp437_mod = _load_module("molt_test_cp437", {str(STDLIB_ROOT / "encodings" / "cp437.py")!r})
koi8_r_mod = _load_module("molt_test_koi8_r", {str(STDLIB_ROOT / "encodings" / "koi8_r.py")!r})

checks = {{
    "johab": johab_mod.getregentry().name == "johab" and "molt_capabilities_has" not in johab_mod.__dict__,
    "iso8859_9": iso8859_9_mod.getregentry().name == "iso8859-9" and "molt_capabilities_has" not in iso8859_9_mod.__dict__,
    "cp1254": cp1254_mod.getregentry().name == "cp1254" and "molt_capabilities_has" not in cp1254_mod.__dict__,
    "gbk": gbk_mod.getregentry().name == "gbk" and "molt_capabilities_has" not in gbk_mod.__dict__,
    "palmos": palmos_mod.getregentry().name == "palmos" and "molt_capabilities_has" not in palmos_mod.__dict__,
    "koi8_u": koi8_u_mod.getregentry().name == "koi8-u" and "molt_capabilities_has" not in koi8_u_mod.__dict__,
    "zlib_codec": zlib_codec_mod.getregentry().name == "zlib" and "molt_capabilities_has" not in zlib_codec_mod.__dict__,
    "utf_32": utf_32_mod.getregentry().name == "utf-32" and "molt_capabilities_has" not in utf_32_mod.__dict__,
    "cp437": cp437_mod.getregentry().name == "cp437" and "molt_capabilities_has" not in cp437_mod.__dict__,
    "koi8_r": koi8_r_mod.getregentry().name == "koi8-r" and "molt_capabilities_has" not in koi8_r_mod.__dict__,
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_public_intrinsic_surface_batch_x() -> None:
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
        "cp1254": "True",
        "cp437": "True",
        "gbk": "True",
        "iso8859_9": "True",
        "johab": "True",
        "koi8_r": "True",
        "koi8_u": "True",
        "palmos": "True",
        "utf_32": "True",
        "zlib_codec": "True",
    }
