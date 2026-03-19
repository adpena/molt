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

chunk_mod = _load_module("molt_test_chunk", {str(STDLIB_ROOT / "chunk.py")!r})
uu_mod = _load_module("molt_test_uu", {str(STDLIB_ROOT / "uu.py")!r})
nturl2path_mod = _load_module("molt_test_nturl2path", {str(STDLIB_ROOT / "nturl2path.py")!r})
curses_ascii_mod = _load_module("molt_test_curses_ascii", {str(STDLIB_ROOT / "curses" / "ascii.py")!r})
robotparser_mod = _load_module("molt_test_robotparser", {str(STDLIB_ROOT / "urllib" / "robotparser.py")!r})
mime_application_mod = _load_module("molt_test_mime_application", {str(STDLIB_ROOT / "email" / "mime" / "application.py")!r})
xml_etree_mod = _load_module("molt_test_elementtree", {str(STDLIB_ROOT / "xml" / "etree" / "ElementTree.py")!r})


def _raises_runtimeerror(mod, attr):
    try:
        getattr(mod, attr)
    except RuntimeError:
        return True
    return False


checks = {{
    "chunk": (
        _raises_runtimeerror(chunk_mod, "Chunk")
        and "molt_capabilities_has" not in chunk_mod.__dict__
    ),
    "uu": (
        _raises_runtimeerror(uu_mod, "encode")
        and "molt_capabilities_has" not in uu_mod.__dict__
    ),
    "nturl2path": (
        _raises_runtimeerror(nturl2path_mod, "url2pathname")
        and "molt_capabilities_has" not in nturl2path_mod.__dict__
    ),
    "curses_ascii": (
        curses_ascii_mod.NUL == 0
        and "molt_capabilities_has" not in curses_ascii_mod.__dict__
    ),
    "robotparser": (
        _raises_runtimeerror(robotparser_mod, "RobotFileParser")
        and "molt_capabilities_has" not in robotparser_mod.__dict__
    ),
    "mime_application": "molt_capabilities_has" not in mime_application_mod.__dict__,
    "xml_etree": (
        _raises_runtimeerror(xml_etree_mod, "ElementTree")
        and "molt_capabilities_has" not in xml_etree_mod.__dict__
    ),
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_public_intrinsic_surface_batch_r() -> None:
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
        "chunk": "True",
        "curses_ascii": "True",
        "mime_application": "True",
        "nturl2path": "True",
        "robotparser": "True",
        "uu": "True",
        "xml_etree": "True",
    }
