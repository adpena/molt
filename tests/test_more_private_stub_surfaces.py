from __future__ import annotations

import subprocess
import sys
from pathlib import Path

import pytest


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"


def _probe_source(module_name: str, display_name: str, module_path: Path) -> str:
    return f"""
import builtins
import importlib.util
import sys
import types

builtins._molt_intrinsics = {{
    "molt_capabilities_has": lambda _name=None: True,
    "molt_import_smoke_runtime_ready": lambda: True,
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

spec = importlib.util.spec_from_file_location({module_name!r}, {str(module_path)!r})
assert spec is not None and spec.loader is not None
module = importlib.util.module_from_spec(spec)
sys.modules[{module_name!r}] = module
spec.loader.exec_module(module)

public_names = [
    name
    for name in sorted(dir(module))
    if not name.startswith("_") and name != "annotations"
]
for name in public_names:
    print(f"ROW|{{name}}")

try:
    getattr(module, "missing_surface")
except RuntimeError as exc:
    expected = {{
        (
            'stdlib module "'
            + {display_name!r}
            + '" is not fully lowered yet; only an intrinsic-first stub is available.'
        )
    }}
    print("CHECK|" + str(next(iter(expected)) in str(exc)))
else:
    print("CHECK|False")
"""


def _run_probe(
    module_name: str, display_name: str, module_path: Path
) -> tuple[list[str], bool]:
    proc = subprocess.run(
        [sys.executable, "-c", _probe_source(module_name, display_name, module_path)],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=True,
    )
    public_names: list[str] = []
    behavior_ok = False
    for line in proc.stdout.splitlines():
        prefix, *rest = line.split("|", 1)
        if prefix == "ROW":
            public_names.append(rest[0])
        elif prefix == "CHECK":
            behavior_ok = rest[0] == "True"
    return public_names, behavior_ok


@pytest.mark.parametrize(
    "module_name,display_name,module_path",
    [
        ("_molt_private_sysconfig", "_sysconfig", STDLIB_ROOT / "_sysconfig.py"),
        ("_molt_private_tracemalloc", "_tracemalloc", STDLIB_ROOT / "_tracemalloc.py"),
        ("_molt_private_string", "_string", STDLIB_ROOT / "_string.py"),
        ("_molt_private_stat", "_stat", STDLIB_ROOT / "_stat.py"),
        ("_molt_private_symtable", "_symtable", STDLIB_ROOT / "_symtable.py"),
        (
            "_molt_private_remote_debugging",
            "_remote_debugging",
            STDLIB_ROOT / "_remote_debugging.py",
        ),
        ("_molt_private_sqlite3", "_sqlite3", STDLIB_ROOT / "_sqlite3.py"),
        ("_molt_private_lsprof", "_lsprof", STDLIB_ROOT / "_lsprof.py"),
        ("_molt_private_pyio", "_pyio", STDLIB_ROOT / "_pyio.py"),
        ("_molt_private_imp", "_imp", STDLIB_ROOT / "_imp.py"),
    ],
)
def test_more_private_stub_surfaces_are_anchor_free(
    module_name: str, display_name: str, module_path: Path
) -> None:
    public_names, behavior_ok = _run_probe(module_name, display_name, module_path)
    assert public_names == []
    assert behavior_ok is True
