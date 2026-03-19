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
        return (bytes(data).decode("utf-8"), len(data))


codecs_jp = types.ModuleType("_codecs_jp")
codecs_jp.getcodec = lambda name: _Codec()
sys.modules["_codecs_jp"] = codecs_jp

codecs_iso2022 = types.ModuleType("_codecs_iso2022")
codecs_iso2022.getcodec = lambda name: _Codec()
sys.modules["_codecs_iso2022"] = codecs_iso2022

iso8859_5_mod = _load_module("molt_test_iso8859_5", {str(STDLIB_ROOT / "encodings" / "iso8859_5.py")!r})
cp1026_mod = _load_module("molt_test_cp1026", {str(STDLIB_ROOT / "encodings" / "cp1026.py")!r})
cp1255_mod = _load_module("molt_test_cp1255", {str(STDLIB_ROOT / "encodings" / "cp1255.py")!r})
shift_jis_2004_mod = _load_module("molt_test_shift_jis_2004", {str(STDLIB_ROOT / "encodings" / "shift_jis_2004.py")!r})
iso2022_jp_3_mod = _load_module("molt_test_iso2022_jp_3", {str(STDLIB_ROOT / "encodings" / "iso2022_jp_3.py")!r})

checks = {{
    "iso8859_5": (
        iso8859_5_mod.getregentry().name == "iso8859-5"
        and "molt_capabilities_has" not in iso8859_5_mod.__dict__
    ),
    "cp1026": (
        cp1026_mod.getregentry().name == "cp1026"
        and "molt_capabilities_has" not in cp1026_mod.__dict__
    ),
    "cp1255": (
        cp1255_mod.getregentry().name == "cp1255"
        and "molt_capabilities_has" not in cp1255_mod.__dict__
    ),
    "shift_jis_2004": (
        shift_jis_2004_mod.getregentry().name == "shift_jis_2004"
        and "molt_capabilities_has" not in shift_jis_2004_mod.__dict__
    ),
    "iso2022_jp_3": (
        iso2022_jp_3_mod.getregentry().name == "iso2022_jp_3"
        and "molt_capabilities_has" not in iso2022_jp_3_mod.__dict__
    ),
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_public_intrinsic_surface_batch_s() -> None:
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
        "cp1026": "True",
        "cp1255": "True",
        "iso2022_jp_3": "True",
        "iso8859_5": "True",
        "shift_jis_2004": "True",
    }
