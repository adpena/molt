from __future__ import annotations

import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"

_PROBE = f"""
import builtins
import codecs as _real_codecs
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

codecs_iso2022 = types.ModuleType("_codecs_iso2022")


class _Codec:
    def encode(self, text, errors="strict"):
        return (text.encode("utf-8"), len(text))

    def decode(self, data, errors="strict"):
        return (bytes(data).decode("utf-8"), len(data))


codecs_iso2022.getcodec = lambda name: _Codec()
sys.modules["_codecs_iso2022"] = codecs_iso2022

enc_charmap_mod = _load_module("molt_test_enc_charmap", {str(STDLIB_ROOT / "encodings" / "charmap.py")!r})
enc_iso2022_jp_mod = _load_module("molt_test_enc_iso2022_jp", {str(STDLIB_ROOT / "encodings" / "iso2022_jp.py")!r})
enc_iso8859_13_mod = _load_module("molt_test_enc_iso8859_13", {str(STDLIB_ROOT / "encodings" / "iso8859_13.py")!r})

charmap_info = enc_charmap_mod.getregentry()
iso2022_info = enc_iso2022_jp_mod.getregentry()

checks = {{
    "enc_charmap": (
        charmap_info.name == "charmap"
        and "molt_capabilities_has" not in enc_charmap_mod.__dict__
    ),
    "enc_iso2022_jp": (
        iso2022_info.name == "iso2022_jp"
        and "molt_capabilities_has" not in enc_iso2022_jp_mod.__dict__
    ),
    "enc_iso8859_13": (
        callable(enc_iso8859_13_mod.getregentry().encode)
        and "molt_capabilities_has" not in enc_iso8859_13_mod.__dict__
    ),
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_public_intrinsic_surface_batch_o() -> None:
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
        "enc_charmap": "True",
        "enc_iso2022_jp": "True",
        "enc_iso8859_13": "True",
    }
