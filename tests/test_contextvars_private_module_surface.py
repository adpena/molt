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


install_registry({{
    "molt_stdlib_probe": lambda: None,
    "molt_cancel_token_get_current": lambda: 1,
}})


def _load_module(name, path_text):
    spec = importlib.util.spec_from_file_location(name, path_text)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


_load_module("contextvars", {str(STDLIB_ROOT / "contextvars.py")!r})
_private = _load_module("_contextvars", {str(STDLIB_ROOT / "_contextvars.py")!r})

rows = [
    (name, type(value).__name__, bool(callable(value)))
    for name, value in sorted(_private.__dict__.items())
    if not name.startswith("_")
]
for name, type_name, is_callable in rows:
    print(f"ROW|{{name}}|{{type_name}}|{{is_callable}}")

var = _private.ContextVar("answer", default=41)
token = var.set(42)
ctx = _private.copy_context()
checks = {{
    "anchor_hidden": "molt_cancel_token_get_current" not in _private.__dict__,
    "behavior": (
        var.get() == 42
        and ctx.get(var) == 42
        and isinstance(token, _private.Token)
        and ctx.run(lambda: var.get()) == 42
    ),
}}
var.reset(token)
checks["reset"] = var.get() == 41
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


def test__contextvars_public_surface_matches_expected_shape() -> None:
    rows, checks = _run_probe()
    assert rows == [
        ("Context", "type", "True"),
        ("ContextVar", "type", "True"),
        ("Token", "type", "True"),
        ("copy_context", "function", "True"),
    ]
    assert checks == {
        "anchor_hidden": "True",
        "behavior": "True",
        "reset": "True",
    }
