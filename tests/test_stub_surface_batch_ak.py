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
    "lib2to3": _load_module("molt_test_lib2to3", {str(STDLIB_ROOT / "lib2to3" / "__init__.py")!r}),
    "lib2to3.fixes.fix_print": _load_module("molt_test_fix_print", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_print.py")!r}),
    "lib2to3.fixes.fix_raise": _load_module("molt_test_fix_raise", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_raise.py")!r}),
    "lib2to3.fixes.fix_raw_input": _load_module("molt_test_fix_raw_input", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_raw_input.py")!r}),
    "lib2to3.fixes.fix_reduce": _load_module("molt_test_fix_reduce", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_reduce.py")!r}),
    "lib2to3.fixes.fix_reload": _load_module("molt_test_fix_reload", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_reload.py")!r}),
    "lib2to3.fixes.fix_renames": _load_module("molt_test_fix_renames", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_renames.py")!r}),
    "lib2to3.fixes.fix_repr": _load_module("molt_test_fix_repr", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_repr.py")!r}),
    "lib2to3.fixes.fix_set_literal": _load_module("molt_test_fix_set_literal", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_set_literal.py")!r}),
    "lib2to3.fixes.fix_standarderror": _load_module("molt_test_fix_standarderror", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_standarderror.py")!r}),
    "lib2to3.fixes.fix_sys_exc": _load_module("molt_test_fix_sys_exc", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_sys_exc.py")!r}),
    "lib2to3.fixes.fix_throw": _load_module("molt_test_fix_throw", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_throw.py")!r}),
    "lib2to3.fixes.fix_tuple_params": _load_module("molt_test_fix_tuple_params", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_tuple_params.py")!r}),
    "lib2to3.fixes.fix_types": _load_module("molt_test_fix_types", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_types.py")!r}),
    "lib2to3.fixes.fix_unicode": _load_module("molt_test_fix_unicode", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_unicode.py")!r}),
    "lib2to3.fixes.fix_urllib": _load_module("molt_test_fix_urllib", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_urllib.py")!r}),
    "lib2to3.fixes.fix_ws_comma": _load_module("molt_test_fix_ws_comma", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_ws_comma.py")!r}),
    "lib2to3.fixes.fix_xrange": _load_module("molt_test_fix_xrange", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_xrange.py")!r}),
    "lib2to3.fixes.fix_xreadlines": _load_module("molt_test_fix_xreadlines", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_xreadlines.py")!r}),
    "lib2to3.fixes.fix_zip": _load_module("molt_test_fix_zip", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_zip.py")!r}),
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


def test_stub_surface_batch_ak() -> None:
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
        "lib2to3": "True",
        "lib2to3.fixes.fix_print": "True",
        "lib2to3.fixes.fix_raise": "True",
        "lib2to3.fixes.fix_raw_input": "True",
        "lib2to3.fixes.fix_reduce": "True",
        "lib2to3.fixes.fix_reload": "True",
        "lib2to3.fixes.fix_renames": "True",
        "lib2to3.fixes.fix_repr": "True",
        "lib2to3.fixes.fix_set_literal": "True",
        "lib2to3.fixes.fix_standarderror": "True",
        "lib2to3.fixes.fix_sys_exc": "True",
        "lib2to3.fixes.fix_throw": "True",
        "lib2to3.fixes.fix_tuple_params": "True",
        "lib2to3.fixes.fix_types": "True",
        "lib2to3.fixes.fix_unicode": "True",
        "lib2to3.fixes.fix_urllib": "True",
        "lib2to3.fixes.fix_ws_comma": "True",
        "lib2to3.fixes.fix_xrange": "True",
        "lib2to3.fixes.fix_xreadlines": "True",
        "lib2to3.fixes.fix_zip": "True",
    }
