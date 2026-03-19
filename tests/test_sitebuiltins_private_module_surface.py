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


_calls = []

builtins._molt_intrinsics = {{
    "molt_site_help0": lambda: _calls.append(("help0", None)),
    "molt_site_help1": lambda topic: _calls.append(("help1", topic)),
    "molt_site_credits": lambda: _calls.append(("credits", None)),
    "molt_site_copyright": lambda: _calls.append(("copyright", None)),
    "molt_site_license": lambda: _calls.append(("license", None)),
    "molt_site_quitter_call": lambda code=None: _calls.append(("quit", code)),
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


_private = _load_module("_molt_private_sitebuiltins", {str(STDLIB_ROOT / "_sitebuiltins.py")!r})

rows = [
    (name, type(getattr(_private, name)).__name__, bool(callable(getattr(_private, name))))
    for name in sorted(dir(_private))
    if not name.startswith("_") and name != "annotations"
]
for name, type_name, is_callable in rows:
    print(f"ROW|{{name}}|{{type_name}}|{{is_callable}}")

_private.help()
_private.help("topic")
_private.credits()
_private.copyright()
_private.license()
_private.quit(7)

checks = {{
    "behavior": (
        _calls
        == [
            ("help0", None),
            ("help1", "topic"),
            ("credits", None),
            ("copyright", None),
            ("license", None),
            ("quit", 7),
        ]
        and "quit()" in repr(_private.quit)
        and "exit()" in repr(_private.exit)
    ),
}}
for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def _run_probe() -> tuple[list[tuple[str, str, str]], dict[str, str]]:
    proc = subprocess.run(
        [sys.executable, "-c", _PROBE],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=True,
    )
    rows: list[tuple[str, str, str]] = []
    checks: dict[str, str] = {}
    for line in proc.stdout.splitlines():
        prefix, *rest = line.split("|")
        if prefix == "ROW":
            rows.append((rest[0], rest[1], rest[2]))
        elif prefix == "CHECK":
            checks[rest[0]] = rest[1]
    return rows, checks


def test__sitebuiltins_public_surface_matches_expected_shape() -> None:
    rows, checks = _run_probe()
    names = [name for name, _, _ in rows]
    assert "molt_site_help0" not in names
    assert "molt_site_help1" not in names
    assert "molt_site_quitter_call" not in names
    assert "help" in names
    assert "credits" in names
    assert "copyright" in names
    assert "license" in names
    assert "quit" in names
    assert "exit" in names
    assert checks == {"behavior": "True"}
