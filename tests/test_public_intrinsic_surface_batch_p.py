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


zoneinfo_private_mod = _load_module("molt_test_zoneinfo_private", {str(STDLIB_ROOT / "zoneinfo" / "_zoneinfo.py")!r})
ctypes_endian_mod = _load_module("molt_test_ctypes_endian", {str(STDLIB_ROOT / "ctypes" / "_endian.py")!r})
ctypes_layout_mod = _load_module("molt_test_ctypes_layout", {str(STDLIB_ROOT / "ctypes" / "_layout.py")!r})
curses_has_key_mod = _load_module("molt_test_curses_has_key", {str(STDLIB_ROOT / "curses" / "has_key.py")!r})
email_mime_audio_mod = _load_module("molt_test_email_mime_audio", {str(STDLIB_ROOT / "email" / "mime" / "audio.py")!r})
email_mime_image_mod = _load_module("molt_test_email_mime_image", {str(STDLIB_ROOT / "email" / "mime" / "image.py")!r})
email_mime_nonmultipart_mod = _load_module("molt_test_email_mime_nonmultipart", {str(STDLIB_ROOT / "email" / "mime" / "nonmultipart.py")!r})


def _raises_runtimeerror(mod, attr):
    try:
        getattr(mod, attr)
    except RuntimeError:
        return True
    return False


checks = {{
    "zoneinfo_private": (
        _raises_runtimeerror(zoneinfo_private_mod, "ZoneInfo")
        and "molt_capabilities_has" not in zoneinfo_private_mod.__dict__
    ),
    "ctypes_endian": (
        hasattr(ctypes_endian_mod, "LittleEndianStructure")
        and "molt_capabilities_has" not in ctypes_endian_mod.__dict__
    ),
    "ctypes_layout": (
        _raises_runtimeerror(ctypes_layout_mod, "StructLayout")
        and "molt_capabilities_has" not in ctypes_layout_mod.__dict__
    ),
    "curses_has_key": (
        curses_has_key_mod.has_key(1) is False
        and "molt_capabilities_has" not in curses_has_key_mod.__dict__
    ),
    "email_mime_audio": "molt_capabilities_has" not in email_mime_audio_mod.__dict__,
    "email_mime_image": "molt_capabilities_has" not in email_mime_image_mod.__dict__,
    "email_mime_nonmultipart": "molt_capabilities_has" not in email_mime_nonmultipart_mod.__dict__,
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_public_intrinsic_surface_batch_p() -> None:
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
        "ctypes_endian": "True",
        "ctypes_layout": "True",
        "curses_has_key": "True",
        "email_mime_audio": "True",
        "email_mime_image": "True",
        "email_mime_nonmultipart": "True",
        "zoneinfo_private": "True",
    }
