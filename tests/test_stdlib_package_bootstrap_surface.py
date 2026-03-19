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


builtins._molt_intrinsics = {{
    "molt_stdlib_probe": lambda: True,
    "molt_capabilities_has": lambda _name=None: True,
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

spec = importlib.util.spec_from_file_location(
    "molt_test_stdlib_pkg", {str(STDLIB_ROOT / "__init__.py")!r}
)
assert spec is not None and spec.loader is not None
module = importlib.util.module_from_spec(spec)
sys.modules["molt_test_stdlib_pkg"] = module
spec.loader.exec_module(module)

checks = {{
    "probe_hidden": "molt_stdlib_probe" not in module.__dict__,
    "cap_hidden": "molt_capabilities_has" not in module.__dict__,
    "cap_callable": callable(module._MOLT_STDLIB_CAP_HAS),
    "intrinsics_alias": module._intrinsics is _intrinsics_mod,
    "intrinsics_registered": sys.modules.get("_intrinsics") is _intrinsics_mod,
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def _run_probe() -> dict[str, str]:
    proc = subprocess.run(
        [sys.executable, "-c", _PROBE],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=True,
    )
    checks: dict[str, str] = {}
    for line in proc.stdout.splitlines():
        prefix, key, value = line.split("|", 2)
        assert prefix == "CHECK"
        checks[key] = value
    return checks


def test_stdlib_package_bootstrap_surface() -> None:
    assert _run_probe() == {
        "cap_callable": "True",
        "cap_hidden": "True",
        "intrinsics_alias": "True",
        "intrinsics_registered": "True",
        "probe_hidden": "True",
    }
