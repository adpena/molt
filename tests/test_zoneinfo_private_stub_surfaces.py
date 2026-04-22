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


def _load_module(name, path_text):
    spec = importlib.util.spec_from_file_location(name, path_text)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


for module_name, path_text in (
    ("molt_test__zoneinfo_common", {str(STDLIB_ROOT / "zoneinfo" / "_common.py")!r}),
    ("molt_test__zoneinfo_tzpath", {str(STDLIB_ROOT / "zoneinfo" / "_tzpath.py")!r}),
):
    module = _load_module(module_name, path_text)
    try:
        getattr(module, "missing_attr")
    except RuntimeError as exc:
        message_ok = "intrinsic-first stub is available" in str(exc)
    else:
        message_ok = False
    print(
        "ROW|"
        f"{{module_name}}|"
        f"{{'molt_capabilities_has' not in module.__dict__}}|"
        f"{{'_MOLT_CAPABILITIES_HAS' not in module.__dict__}}|"
        f"{{message_ok}}"
    )
"""


def test_zoneinfo_private_stub_surfaces_hide_capability_anchor() -> None:
    proc = subprocess.run(
        [sys.executable, "-c", _PROBE],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=True,
    )
    rows = [
        line.split("|")[1:]
        for line in proc.stdout.splitlines()
        if line.startswith("ROW|")
    ]
    assert rows == [
        ["molt_test__zoneinfo_common", "True", "True", "True"],
        ["molt_test__zoneinfo_tzpath", "True", "True", "True"],
    ]
