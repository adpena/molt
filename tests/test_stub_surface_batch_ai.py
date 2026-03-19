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
    "_pyrepl.types": _load_module("molt_test__pyrepl_types", {str(STDLIB_ROOT / "_pyrepl" / "types.py")!r}),
    "_pyrepl.unix_console": _load_module("molt_test__pyrepl_unix_console", {str(STDLIB_ROOT / "_pyrepl" / "unix_console.py")!r}),
    "_pyrepl.unix_eventqueue": _load_module("molt_test__pyrepl_unix_eventqueue", {str(STDLIB_ROOT / "_pyrepl" / "unix_eventqueue.py")!r}),
    "_pyrepl.utils": _load_module("molt_test__pyrepl_utils", {str(STDLIB_ROOT / "_pyrepl" / "utils.py")!r}),
    "_pyrepl.windows_console": _load_module("molt_test__pyrepl_windows_console", {str(STDLIB_ROOT / "_pyrepl" / "windows_console.py")!r}),
    "_pyrepl.windows_eventqueue": _load_module("molt_test__pyrepl_windows_eventqueue", {str(STDLIB_ROOT / "_pyrepl" / "windows_eventqueue.py")!r}),
    "lib2to3.fixes": _load_module("molt_test_lib2to3_fixes", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "__init__.py")!r}),
    "lib2to3.fixes.fix_apply": _load_module("molt_test_lib2to3_fix_apply", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_apply.py")!r}),
    "lib2to3.fixes.fix_asserts": _load_module("molt_test_lib2to3_fix_asserts", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_asserts.py")!r}),
    "lib2to3.fixes.fix_basestring": _load_module("molt_test_lib2to3_fix_basestring", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_basestring.py")!r}),
    "lib2to3.fixes.fix_buffer": _load_module("molt_test_lib2to3_fix_buffer", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_buffer.py")!r}),
    "lib2to3.fixes.fix_dict": _load_module("molt_test_lib2to3_fix_dict", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_dict.py")!r}),
    "lib2to3.fixes.fix_except": _load_module("molt_test_lib2to3_fix_except", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_except.py")!r}),
    "lib2to3.fixes.fix_exec": _load_module("molt_test_lib2to3_fix_exec", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_exec.py")!r}),
    "lib2to3.fixes.fix_execfile": _load_module("molt_test_lib2to3_fix_execfile", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_execfile.py")!r}),
    "lib2to3.fixes.fix_exitfunc": _load_module("molt_test_lib2to3_fix_exitfunc", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_exitfunc.py")!r}),
    "lib2to3.fixes.fix_filter": _load_module("molt_test_lib2to3_fix_filter", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_filter.py")!r}),
    "lib2to3.fixes.fix_funcattrs": _load_module("molt_test_lib2to3_fix_funcattrs", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_funcattrs.py")!r}),
    "lib2to3.fixes.fix_future": _load_module("molt_test_lib2to3_fix_future", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_future.py")!r}),
    "lib2to3.fixes.fix_getcwdu": _load_module("molt_test_lib2to3_fix_getcwdu", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_getcwdu.py")!r}),
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


def test_stub_surface_batch_ai() -> None:
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
        "_pyrepl.types": "True",
        "_pyrepl.unix_console": "True",
        "_pyrepl.unix_eventqueue": "True",
        "_pyrepl.utils": "True",
        "_pyrepl.windows_console": "True",
        "_pyrepl.windows_eventqueue": "True",
        "lib2to3.fixes": "True",
        "lib2to3.fixes.fix_apply": "True",
        "lib2to3.fixes.fix_asserts": "True",
        "lib2to3.fixes.fix_basestring": "True",
        "lib2to3.fixes.fix_buffer": "True",
        "lib2to3.fixes.fix_dict": "True",
        "lib2to3.fixes.fix_except": "True",
        "lib2to3.fixes.fix_exec": "True",
        "lib2to3.fixes.fix_execfile": "True",
        "lib2to3.fixes.fix_exitfunc": "True",
        "lib2to3.fixes.fix_filter": "True",
        "lib2to3.fixes.fix_funcattrs": "True",
        "lib2to3.fixes.fix_future": "True",
        "lib2to3.fixes.fix_getcwdu": "True",
    }
