from __future__ import annotations

import sys
from pathlib import Path

from tests.surface_process_guard import run_surface_test_process


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"

_PROBE = f"""
import importlib.util
import sys
from tests.stdlib_intrinsic_registry import install_registry


# No intrinsic at all: none of these modules may require one.
install_registry({{}}, with_capabilities=False)


def _load_module(name, path_text):
    spec = importlib.util.spec_from_file_location(name, path_text)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


grp = _load_module("grp", {str(STDLIB_ROOT / "grp.py")!r})
pyclbr = _load_module("pyclbr", {str(STDLIB_ROOT / "pyclbr.py")!r})
sre_parse = _load_module("sre_parse", {str(STDLIB_ROOT / "sre_parse.py")!r})
parsed = sre_parse.parse("abc")

checks = {{
    "behavior": parsed == [],
    "private_handles_hidden": (
        "_MOLT_IMPORT_SMOKE_RUNTIME_READY" not in grp.__dict__
        and "_MOLT_IMPORT_SMOKE_RUNTIME_READY" not in pyclbr.__dict__
        and "_MOLT_IMPORT_SMOKE_RUNTIME_READY" not in sre_parse.__dict__
        and "molt_import_smoke_runtime_ready" not in grp.__dict__
        and "molt_import_smoke_runtime_ready" not in pyclbr.__dict__
        and "molt_import_smoke_runtime_ready" not in sre_parse.__dict__
    ),
}}
for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_import_smoke_public_modules_hide_bootstrap_handles() -> None:
    proc = run_surface_test_process(
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
    assert checks == {"behavior": "True", "private_handles_hidden": "True"}
