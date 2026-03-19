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


turtle_mod = _load_module("molt_test_turtle", {str(STDLIB_ROOT / "turtle.py")!r})
curses_panel_mod = _load_module("molt_test_curses_panel", {str(STDLIB_ROOT / "curses" / "panel.py")!r})
tomllib_re_mod = _load_module("molt_test_tomllib_re", {str(STDLIB_ROOT / "tomllib" / "_re.py")!r})
tomllib_parser_mod = _load_module("molt_test_tomllib_parser", {str(STDLIB_ROOT / "tomllib" / "_parser.py")!r})
tomllib_types_mod = _load_module("molt_test_tomllib_types", {str(STDLIB_ROOT / "tomllib" / "_types.py")!r})
wsgiref_handlers_mod = _load_module("molt_test_wsgiref_handlers", {str(STDLIB_ROOT / "wsgiref" / "handlers.py")!r})


def _raises_runtimeerror(mod, attr):
    try:
        getattr(mod, attr)
    except RuntimeError:
        return True
    return False


checks = {{
    "turtle": (
        _raises_runtimeerror(turtle_mod, "Turtle")
        and "molt_capabilities_has" not in turtle_mod.__dict__
    ),
    "curses_panel": (
        curses_panel_mod.version == "2.0"
        and "molt_capabilities_has" not in curses_panel_mod.__dict__
    ),
    "tomllib_re": (
        _raises_runtimeerror(tomllib_re_mod, "compile")
        and "molt_capabilities_has" not in tomllib_re_mod.__dict__
    ),
    "tomllib_parser": (
        _raises_runtimeerror(tomllib_parser_mod, "loads")
        and "molt_capabilities_has" not in tomllib_parser_mod.__dict__
    ),
    "tomllib_types": (
        _raises_runtimeerror(tomllib_types_mod, "ParseError")
        and "molt_capabilities_has" not in tomllib_types_mod.__dict__
    ),
    "wsgiref_handlers": (
        _raises_runtimeerror(wsgiref_handlers_mod, "SimpleHandler")
        and "molt_capabilities_has" not in wsgiref_handlers_mod.__dict__
    ),
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_public_intrinsic_surface_batch_j() -> None:
    proc = subprocess.run(
        [sys.executable, "-c", _PROBE],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=True,
    )
    checks: dict[str, str] = {}
    for line in proc.stdout.splitlines():
        prefix, *rest = line.split("|")
        if prefix == "CHECK":
            checks[rest[0]] = rest[1]
    assert checks == {
        "curses_panel": "True",
        "tomllib_parser": "True",
        "tomllib_re": "True",
        "tomllib_types": "True",
        "turtle": "True",
        "wsgiref_handlers": "True",
    }
