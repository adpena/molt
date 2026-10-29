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


def _warn(message, category=None, stacklevel=1, source=None):
    return ("warn", str(message), category.__name__ if category else None, int(stacklevel))


def _warn_explicit(message, category, filename, lineno, module=None, registry=None, module_globals=None, source=None):
    return ("warn_explicit", str(message), getattr(category, "__name__", None), str(filename), int(lineno))


install_registry({{
    "molt_warnings_warn": _warn,
    "molt_warnings_warn_explicit": _warn_explicit,
    "molt_warnings_formatwarning": lambda *args: "formatted",
    "molt_warnings_simplefilter": lambda *args: None,
    "molt_warnings_filterwarnings": lambda *args: None,
    "molt_warnings_resetwarnings": lambda *args: None,
    "molt_warnings_filters_get": lambda: [("default", None, Warning, None, 0)],
    "molt_warnings_onceregistry_get": dict,
    "molt_warnings_filters_mutated": lambda: None,
}})


def _load_module(name, path_text):
    spec = importlib.util.spec_from_file_location(name, path_text)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


# `_warnings` is the provider and never imports `warnings`, its consumer.
sys.modules.pop("warnings", None)
_private = _load_module("_warnings", {str(STDLIB_ROOT / "_warnings.py")!r})
_provider_loaded_consumer = "warnings" in sys.modules
_warnings_mod = _load_module("warnings", {str(STDLIB_ROOT / "warnings.py")!r})

rows = [
    (name, type(value).__name__, bool(callable(value)))
    for name, value in sorted(_private.__dict__.items())
    if not name.startswith("_")
]
for name, type_name, is_callable in rows:
    print(f"ROW|{{name}}|{{type_name}}|{{is_callable}}")

checks = {{
    "aliases": (
        _private.filters is _warnings_mod.filters
        and _private._onceregistry is _warnings_mod.onceregistry
        and _private._defaultaction == _warnings_mod.defaultaction
        and _private._filters_mutated is _warnings_mod._filters_mutated
        and _private.warn is _warnings_mod.warn
        and _private.warn_explicit is _warnings_mod.warn_explicit
    ),
    "filters": isinstance(_private.filters, list),
    "provider_loaded_consumer": _provider_loaded_consumer,
    "warn": _private.warn("hello", UserWarning, 2) is None,
    "warn_explicit": _private.warn_explicit("boom", UserWarning, "file.py", 7) is None,
    "warnings_module_private_handles_hidden": (
        "_MOLT_STDLIB_PROBE" not in sys.modules["warnings"].__dict__
        and "_MOLT_CAPABILITIES_HAS" not in sys.modules["warnings"].__dict__
        and "molt_stdlib_probe" not in sys.modules["warnings"].__dict__
        and "molt_capabilities_has" not in sys.modules["warnings"].__dict__
        and "molt_warnings_warn" not in sys.modules["warnings"].__dict__
        and "molt_warnings_warn_explicit" not in sys.modules["warnings"].__dict__
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


def test__warnings_public_surface_matches_expected_shape() -> None:
    rows, checks = _run_probe()
    assert rows == [
        ("filters", "list", "False"),
        ("warn", "function", "True"),
        ("warn_explicit", "function", "True"),
    ]
    assert checks == {
        "aliases": "True",
        "filters": "True",
        "provider_loaded_consumer": "False",
        "warn": "True",
        "warn_explicit": "True",
        "warnings_module_private_handles_hidden": "True",
    }
