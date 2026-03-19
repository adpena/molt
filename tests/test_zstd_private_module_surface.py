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

spec = importlib.util.spec_from_file_location(
    "molt_test__zstd", {str(STDLIB_ROOT / "_zstd.py")!r}
)
assert spec is not None and spec.loader is not None
module = importlib.util.module_from_spec(spec)
sys.modules["molt_test__zstd"] = module
spec.loader.exec_module(module)

public_names = [
    name
    for name in sorted(dir(module))
    if not name.startswith("_") and name != "annotations"
]
for name in public_names:
    print(f"ROW|{{name}}")

try:
    module.ZstdCompressor
except RuntimeError as exc:
    behavior_ok = (
        'stdlib module "_zstd" is not fully lowered yet; only an intrinsic-first stub is available.'
        in str(exc)
    )
else:
    behavior_ok = False

print("CHECK|anchor_hidden|" + str("molt_capabilities_has" not in module.__dict__))
print("CHECK|behavior|" + str(behavior_ok))
"""


def _run_probe() -> tuple[list[str], dict[str, str]]:
    proc = subprocess.run(
        [sys.executable, "-c", _PROBE],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=True,
    )
    rows: list[str] = []
    checks: dict[str, str] = {}
    for line in proc.stdout.splitlines():
        prefix, *rest = line.split("|", 2)
        if prefix == "ROW":
            rows.append(rest[0])
        elif prefix == "CHECK":
            checks[rest[0]] = rest[1]
    return rows, checks


def test_zstd_private_module_surface() -> None:
    rows, checks = _run_probe()
    assert rows == []
    assert checks == {"anchor_hidden": "True", "behavior": "True"}
