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


codecs_iso2022 = types.ModuleType("_codecs_iso2022")
codecs_iso2022.getcodec = lambda name: _Codec()
sys.modules["_codecs_iso2022"] = codecs_iso2022

codecs_kr = types.ModuleType("_codecs_kr")
codecs_kr.getcodec = lambda name: _Codec()
sys.modules["_codecs_kr"] = codecs_kr

codecs_jp = types.ModuleType("_codecs_jp")
codecs_jp.getcodec = lambda name: _Codec()
sys.modules["_codecs_jp"] = codecs_jp

codecs_tw = types.ModuleType("_codecs_tw")
codecs_tw.getcodec = lambda name: _Codec()
sys.modules["_codecs_tw"] = codecs_tw

iso2022_jp_ext_mod = _load_module("molt_test_iso2022_jp_ext", {str(STDLIB_ROOT / "encodings" / "iso2022_jp_ext.py")!r})
iso2022_jp_1_mod = _load_module("molt_test_iso2022_jp_1", {str(STDLIB_ROOT / "encodings" / "iso2022_jp_1.py")!r})
iso2022_jp_2_mod = _load_module("molt_test_iso2022_jp_2", {str(STDLIB_ROOT / "encodings" / "iso2022_jp_2.py")!r})
iso2022_jp_2004_mod = _load_module("molt_test_iso2022_jp_2004", {str(STDLIB_ROOT / "encodings" / "iso2022_jp_2004.py")!r})
cp949_mod = _load_module("molt_test_cp949", {str(STDLIB_ROOT / "encodings" / "cp949.py")!r})
big5_mod = _load_module("molt_test_big5", {str(STDLIB_ROOT / "encodings" / "big5.py")!r})
iso2022_kr_mod = _load_module("molt_test_iso2022_kr", {str(STDLIB_ROOT / "encodings" / "iso2022_kr.py")!r})
euc_jisx0213_mod = _load_module("molt_test_euc_jisx0213", {str(STDLIB_ROOT / "encodings" / "euc_jisx0213.py")!r})
euc_kr_mod = _load_module("molt_test_euc_kr", {str(STDLIB_ROOT / "encodings" / "euc_kr.py")!r})
shift_jis_mod = _load_module("molt_test_shift_jis", {str(STDLIB_ROOT / "encodings" / "shift_jis.py")!r})

checks = {{
    "iso2022_jp_ext": iso2022_jp_ext_mod.getregentry().name == "iso2022_jp_ext" and "molt_capabilities_has" not in iso2022_jp_ext_mod.__dict__,
    "iso2022_jp_1": iso2022_jp_1_mod.getregentry().name == "iso2022_jp_1" and "molt_capabilities_has" not in iso2022_jp_1_mod.__dict__,
    "iso2022_jp_2": iso2022_jp_2_mod.getregentry().name == "iso2022_jp_2" and "molt_capabilities_has" not in iso2022_jp_2_mod.__dict__,
    "iso2022_jp_2004": iso2022_jp_2004_mod.getregentry().name == "iso2022_jp_2004" and "molt_capabilities_has" not in iso2022_jp_2004_mod.__dict__,
    "cp949": cp949_mod.getregentry().name == "cp949" and "molt_capabilities_has" not in cp949_mod.__dict__,
    "big5": big5_mod.getregentry().name == "big5" and "molt_capabilities_has" not in big5_mod.__dict__,
    "iso2022_kr": iso2022_kr_mod.getregentry().name == "iso2022_kr" and "molt_capabilities_has" not in iso2022_kr_mod.__dict__,
    "euc_jisx0213": euc_jisx0213_mod.getregentry().name == "euc_jisx0213" and "molt_capabilities_has" not in euc_jisx0213_mod.__dict__,
    "euc_kr": euc_kr_mod.getregentry().name == "euc_kr" and "molt_capabilities_has" not in euc_kr_mod.__dict__,
    "shift_jis": shift_jis_mod.getregentry().name == "shift_jis" and "molt_capabilities_has" not in shift_jis_mod.__dict__,
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_public_intrinsic_surface_batch_ad() -> None:
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
        "big5": "True",
        "cp949": "True",
        "euc_jisx0213": "True",
        "euc_kr": "True",
        "iso2022_jp_1": "True",
        "iso2022_jp_2": "True",
        "iso2022_jp_2004": "True",
        "iso2022_jp_ext": "True",
        "iso2022_kr": "True",
        "shift_jis": "True",
    }
