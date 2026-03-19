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
    "molt_punycode_encode": lambda text: str(text).encode("ascii", "ignore"),
    "molt_punycode_decode": lambda text, errors="strict": bytes(text).decode("ascii", errors),
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

codecs_tw = types.ModuleType("_codecs_tw")
codecs_tw.getcodec = lambda name: _Codec()
sys.modules["_codecs_tw"] = codecs_tw

ptcp154_mod = _load_module("molt_test_ptcp154", {str(STDLIB_ROOT / "encodings" / "ptcp154.py")!r})
cp932_mod = _load_module("molt_test_cp932", {str(STDLIB_ROOT / "encodings" / "cp932.py")!r})
cp950_mod = _load_module("molt_test_cp950", {str(STDLIB_ROOT / "encodings" / "cp950.py")!r})
shift_jisx0213_mod = _load_module("molt_test_shift_jisx0213", {str(STDLIB_ROOT / "encodings" / "shift_jisx0213.py")!r})
raw_unicode_escape_mod = _load_module("molt_test_raw_unicode_escape", {str(STDLIB_ROOT / "encodings" / "raw_unicode_escape.py")!r})
unicode_escape_mod = _load_module("molt_test_unicode_escape", {str(STDLIB_ROOT / "encodings" / "unicode_escape.py")!r})
rot_13_mod = _load_module("molt_test_rot_13", {str(STDLIB_ROOT / "encodings" / "rot_13.py")!r})
punycode_mod = _load_module("molt_test_punycode", {str(STDLIB_ROOT / "encodings" / "punycode.py")!r})
undefined_mod = _load_module("molt_test_undefined", {str(STDLIB_ROOT / "encodings" / "undefined.py")!r})
idna_mod = _load_module("molt_test_idna", {str(STDLIB_ROOT / "encodings" / "idna.py")!r})

checks = {{
    "ptcp154": ptcp154_mod.getregentry().name == "ptcp154" and "molt_capabilities_has" not in ptcp154_mod.__dict__,
    "cp932": cp932_mod.getregentry().name == "cp932" and "molt_capabilities_has" not in cp932_mod.__dict__,
    "cp950": cp950_mod.getregentry().name == "cp950" and "molt_capabilities_has" not in cp950_mod.__dict__,
    "shift_jisx0213": shift_jisx0213_mod.getregentry().name == "shift_jisx0213" and "molt_capabilities_has" not in shift_jisx0213_mod.__dict__,
    "raw_unicode_escape": raw_unicode_escape_mod.getregentry().name == "raw-unicode-escape" and "molt_capabilities_has" not in raw_unicode_escape_mod.__dict__,
    "unicode_escape": unicode_escape_mod.getregentry().name == "unicode-escape" and "molt_capabilities_has" not in unicode_escape_mod.__dict__,
    "rot_13": rot_13_mod.getregentry().name == "rot-13" and "molt_capabilities_has" not in rot_13_mod.__dict__,
    "punycode": punycode_mod.getregentry().name == "punycode"
    and "molt_capabilities_has" not in punycode_mod.__dict__
    and "molt_punycode_encode" not in punycode_mod.__dict__
    and "molt_punycode_decode" not in punycode_mod.__dict__,
    "undefined": undefined_mod.getregentry().name == "undefined" and "molt_capabilities_has" not in undefined_mod.__dict__,
    "idna": idna_mod.getregentry().name == "idna" and "molt_capabilities_has" not in idna_mod.__dict__,
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_public_intrinsic_surface_batch_ah() -> None:
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
        "cp932": "True",
        "cp950": "True",
        "idna": "True",
        "ptcp154": "True",
        "punycode": "True",
        "raw_unicode_escape": "True",
        "rot_13": "True",
        "shift_jisx0213": "True",
        "undefined": "True",
        "unicode_escape": "True",
    }
