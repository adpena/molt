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


class ReferenceType:
    def __init__(self, obj):
        self.obj = obj

    def __call__(self):
        return self.obj


def getweakrefcount(obj):
    return 7


def getweakrefs(obj):
    return [("ref", obj)]


install_registry({{
    "molt_weakref_count": lambda obj: 7,
    "molt_weakref_refs": getweakrefs,
    "molt_weakref_reference_type": lambda: ReferenceType,
}})


def _load_module(name, path_text):
    spec = importlib.util.spec_from_file_location(name, path_text)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


_private = _load_module("_weakref", {str(STDLIB_ROOT / "_weakref.py")!r})

rows = [
    (name, type(getattr(_private, name)).__name__, bool(callable(getattr(_private, name))))
    for name in sorted(dir(_private))
    if not name.startswith("_") and name != "annotations"
]
for name, type_name, is_callable in rows:
    print(f"ROW|{{name}}|{{type_name}}|{{is_callable}}")

checks = {{
    "anchor_hidden": "molt_weakref_count" not in _private.__dict__,
    "behavior": (
        _private.ref is _private.ReferenceType
        and _private.ref("x")() == "x"
        and _private.getweakrefcount("x") == 7
        and _private.getweakrefs("x") == [("ref", "x")]
    ),
}}
for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def _run_probe() -> tuple[list[tuple[str, str, str]], dict[str, str]]:
    proc = run_surface_test_process(
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


def test__weakref_public_surface_matches_expected_shape() -> None:
    rows, checks = _run_probe()
    assert rows == [
        ("ReferenceType", "type", "True"),
        ("getweakrefcount", "function", "True"),
        ("getweakrefs", "function", "True"),
        ("ref", "type", "True"),
    ]
    assert checks == {"anchor_hidden": "True", "behavior": "True"}
