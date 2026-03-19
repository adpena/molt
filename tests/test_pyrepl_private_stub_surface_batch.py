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

modules = {{
    "_pyrepl": _load_module("molt_test__pyrepl", {str(STDLIB_ROOT / "_pyrepl" / "__init__.py")!r}),
    "_pyrepl.__main__": _load_module("molt_test__pyrepl___main__", {str(STDLIB_ROOT / "_pyrepl" / "__main__.py")!r}),
    "_pyrepl._minimal_curses": _load_module("molt_test__pyrepl__minimal_curses", {str(STDLIB_ROOT / "_pyrepl" / "_minimal_curses.py")!r}),
    "_pyrepl._module_completer": _load_module("molt_test__pyrepl__module_completer", {str(STDLIB_ROOT / "_pyrepl" / "_module_completer.py")!r}),
    "_pyrepl._threading_handler": _load_module("molt_test__pyrepl__threading_handler", {str(STDLIB_ROOT / "_pyrepl" / "_threading_handler.py")!r}),
    "_pyrepl.base_eventqueue": _load_module("molt_test__pyrepl_base_eventqueue", {str(STDLIB_ROOT / "_pyrepl" / "base_eventqueue.py")!r}),
    "_pyrepl.commands": _load_module("molt_test__pyrepl_commands", {str(STDLIB_ROOT / "_pyrepl" / "commands.py")!r}),
    "_pyrepl.completing_reader": _load_module("molt_test__pyrepl_completing_reader", {str(STDLIB_ROOT / "_pyrepl" / "completing_reader.py")!r}),
    "_pyrepl.console": _load_module("molt_test__pyrepl_console", {str(STDLIB_ROOT / "_pyrepl" / "console.py")!r}),
    "_pyrepl.curses": _load_module("molt_test__pyrepl_curses", {str(STDLIB_ROOT / "_pyrepl" / "curses.py")!r}),
    "_pyrepl.fancy_termios": _load_module("molt_test__pyrepl_fancy_termios", {str(STDLIB_ROOT / "_pyrepl" / "fancy_termios.py")!r}),
    "_pyrepl.historical_reader": _load_module("molt_test__pyrepl_historical_reader", {str(STDLIB_ROOT / "_pyrepl" / "historical_reader.py")!r}),
    "_pyrepl.input": _load_module("molt_test__pyrepl_input", {str(STDLIB_ROOT / "_pyrepl" / "input.py")!r}),
    "_pyrepl.keymap": _load_module("molt_test__pyrepl_keymap", {str(STDLIB_ROOT / "_pyrepl" / "keymap.py")!r}),
    "_pyrepl.main": _load_module("molt_test__pyrepl_main", {str(STDLIB_ROOT / "_pyrepl" / "main.py")!r}),
    "_pyrepl.pager": _load_module("molt_test__pyrepl_pager", {str(STDLIB_ROOT / "_pyrepl" / "pager.py")!r}),
    "_pyrepl.reader": _load_module("molt_test__pyrepl_reader", {str(STDLIB_ROOT / "_pyrepl" / "reader.py")!r}),
    "_pyrepl.readline": _load_module("molt_test__pyrepl_readline", {str(STDLIB_ROOT / "_pyrepl" / "readline.py")!r}),
    "_pyrepl.terminfo": _load_module("molt_test__pyrepl_terminfo", {str(STDLIB_ROOT / "_pyrepl" / "terminfo.py")!r}),
    "_pyrepl.trace": _load_module("molt_test__pyrepl_trace", {str(STDLIB_ROOT / "_pyrepl" / "trace.py")!r}),
}}

checks = {{}}
for name, module in modules.items():
    try:
        getattr(module, "sentinel")
    except RuntimeError as exc:
        checks[name] = (
            "only an intrinsic-first stub is available" in str(exc)
            and "molt_capabilities_has" not in module.__dict__
        )
    else:
        checks[name] = False

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_pyrepl_private_stub_surface_batch() -> None:
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
        "_pyrepl": "True",
        "_pyrepl.__main__": "True",
        "_pyrepl._minimal_curses": "True",
        "_pyrepl._module_completer": "True",
        "_pyrepl._threading_handler": "True",
        "_pyrepl.base_eventqueue": "True",
        "_pyrepl.commands": "True",
        "_pyrepl.completing_reader": "True",
        "_pyrepl.console": "True",
        "_pyrepl.curses": "True",
        "_pyrepl.fancy_termios": "True",
        "_pyrepl.historical_reader": "True",
        "_pyrepl.input": "True",
        "_pyrepl.keymap": "True",
        "_pyrepl.main": "True",
        "_pyrepl.pager": "True",
        "_pyrepl.reader": "True",
        "_pyrepl.readline": "True",
        "_pyrepl.terminfo": "True",
        "_pyrepl.trace": "True",
    }
