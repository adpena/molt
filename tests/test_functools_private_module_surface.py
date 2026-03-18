from __future__ import annotations

import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"

_PROBE = f"""
import builtins
import functools as _host_functools
import importlib.util
import sys
import types

builtins._molt_intrinsics = {{
    "molt_functools_kwd_mark": lambda: getattr(_host_functools, "Placeholder", object()),
    "molt_functools_update_wrapper": _host_functools.update_wrapper,
    "molt_functools_wraps": _host_functools.wraps,
    "molt_functools_partial": lambda func, args, kwargs: _host_functools.partial(func, *args, **kwargs),
    "molt_functools_reduce": _host_functools.reduce,
    "molt_functools_lru_cache": _host_functools.lru_cache,
    "molt_functools_cmp_to_key": _host_functools.cmp_to_key,
    "molt_functools_total_ordering": _host_functools.total_ordering,
    "molt_functools_singledispatch_new": lambda func: ("sd", func),
    "molt_functools_singledispatch_register": lambda *args: None,
    "molt_functools_singledispatch_call": lambda handle, args, kwargs: handle[1],
    "molt_functools_singledispatch_dispatch": lambda handle, cls: handle[1],
    "molt_functools_singledispatch_registry": lambda handle: {{}},
    "molt_functools_singledispatch_drop": lambda handle: None,
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


_private = _load_module("_functools", {str(STDLIB_ROOT / "_functools.py")!r})

rows = [
    (name, type(value).__name__, bool(callable(value)))
    for name, value in sorted(_private.__dict__.items())
    if not name.startswith("_")
]
for name, type_name, is_callable in rows:
    print(f"ROW|{{name}}|{{type_name}}|{{is_callable}}")

checks = {{
    "aliases": (
        _private.cmp_to_key is _host_functools.cmp_to_key
        and _private.reduce is _host_functools.reduce
        and _private.partial is type(_host_functools.partial(lambda: None))
    ),
    "placeholder_gate": (
        hasattr(_private, "Placeholder") == hasattr(_host_functools, "Placeholder")
    ),
}}
if hasattr(_host_functools, "Placeholder"):
    checks["aliases"] = (
        checks["aliases"]
        and _private.Placeholder is _host_functools.Placeholder
        and _private.partial(lambda a, b, c: (a, b, c), _private.Placeholder, 2)(1, 3) == (1, 2, 3)
    )
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


def test__functools_public_surface_matches_expected_shape() -> None:
    rows, checks = _run_probe()
    expected = [
        ("cmp_to_key", "builtin_function_or_method", "True"),
        ("partial", "type", "True"),
        ("reduce", "builtin_function_or_method", "True"),
    ]
    if sys.version_info >= (3, 14):
        expected.insert(0, ("Placeholder", "_PlaceholderType", "False"))
    assert rows == expected
    assert checks == {"aliases": "True", "placeholder_gate": "True"}
