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
    "lib2to3.fixes.fix_has_key": _load_module("molt_test_fix_has_key", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_has_key.py")!r}),
    "lib2to3.fixes.fix_idioms": _load_module("molt_test_fix_idioms", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_idioms.py")!r}),
    "lib2to3.fixes.fix_import": _load_module("molt_test_fix_import", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_import.py")!r}),
    "lib2to3.fixes.fix_imports": _load_module("molt_test_fix_imports", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_imports.py")!r}),
    "lib2to3.fixes.fix_imports2": _load_module("molt_test_fix_imports2", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_imports2.py")!r}),
    "lib2to3.fixes.fix_input": _load_module("molt_test_fix_input", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_input.py")!r}),
    "lib2to3.fixes.fix_intern": _load_module("molt_test_fix_intern", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_intern.py")!r}),
    "lib2to3.fixes.fix_isinstance": _load_module("molt_test_fix_isinstance", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_isinstance.py")!r}),
    "lib2to3.fixes.fix_itertools": _load_module("molt_test_fix_itertools", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_itertools.py")!r}),
    "lib2to3.fixes.fix_itertools_imports": _load_module("molt_test_fix_itertools_imports", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_itertools_imports.py")!r}),
    "lib2to3.fixes.fix_long": _load_module("molt_test_fix_long", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_long.py")!r}),
    "lib2to3.fixes.fix_map": _load_module("molt_test_fix_map", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_map.py")!r}),
    "lib2to3.fixes.fix_metaclass": _load_module("molt_test_fix_metaclass", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_metaclass.py")!r}),
    "lib2to3.fixes.fix_methodattrs": _load_module("molt_test_fix_methodattrs", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_methodattrs.py")!r}),
    "lib2to3.fixes.fix_ne": _load_module("molt_test_fix_ne", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_ne.py")!r}),
    "lib2to3.fixes.fix_next": _load_module("molt_test_fix_next", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_next.py")!r}),
    "lib2to3.fixes.fix_nonzero": _load_module("molt_test_fix_nonzero", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_nonzero.py")!r}),
    "lib2to3.fixes.fix_numliterals": _load_module("molt_test_fix_numliterals", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_numliterals.py")!r}),
    "lib2to3.fixes.fix_operator": _load_module("molt_test_fix_operator", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_operator.py")!r}),
    "lib2to3.fixes.fix_paren": _load_module("molt_test_fix_paren", {str(STDLIB_ROOT / "lib2to3" / "fixes" / "fix_paren.py")!r}),
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


def test_stub_surface_batch_aj() -> None:
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
        "lib2to3.fixes.fix_has_key": "True",
        "lib2to3.fixes.fix_idioms": "True",
        "lib2to3.fixes.fix_import": "True",
        "lib2to3.fixes.fix_imports": "True",
        "lib2to3.fixes.fix_imports2": "True",
        "lib2to3.fixes.fix_input": "True",
        "lib2to3.fixes.fix_intern": "True",
        "lib2to3.fixes.fix_isinstance": "True",
        "lib2to3.fixes.fix_itertools": "True",
        "lib2to3.fixes.fix_itertools_imports": "True",
        "lib2to3.fixes.fix_long": "True",
        "lib2to3.fixes.fix_map": "True",
        "lib2to3.fixes.fix_metaclass": "True",
        "lib2to3.fixes.fix_methodattrs": "True",
        "lib2to3.fixes.fix_ne": "True",
        "lib2to3.fixes.fix_next": "True",
        "lib2to3.fixes.fix_nonzero": "True",
        "lib2to3.fixes.fix_numliterals": "True",
        "lib2to3.fixes.fix_operator": "True",
        "lib2to3.fixes.fix_paren": "True",
    }
