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


class Generic:
    pass


class ParamSpec:
    pass


class ParamSpecArgs:
    pass


class ParamSpecKwargs:
    pass


class TypeAliasType:
    pass


class TypeVar:
    pass


class TypeVarTuple:
    pass


_fake_typing = types.ModuleType("typing")
for name, value in {{
    "Generic": Generic,
    "ParamSpec": ParamSpec,
    "ParamSpecArgs": ParamSpecArgs,
    "ParamSpecKwargs": ParamSpecKwargs,
    "TypeAliasType": TypeAliasType,
    "TypeVar": TypeVar,
    "TypeVarTuple": TypeVarTuple,
}}.items():
    setattr(_fake_typing, name, value)
sys.modules["typing"] = _fake_typing


def _typing_private_payload(_typing_module):
    return {{
        "Generic": Generic,
        "ParamSpec": ParamSpec,
        "ParamSpecArgs": ParamSpecArgs,
        "ParamSpecKwargs": ParamSpecKwargs,
        "TypeAliasType": TypeAliasType,
        "TypeVar": TypeVar,
        "TypeVarTuple": TypeVarTuple,
    }}


builtins._molt_intrinsics = {{
    "molt_stdlib_probe": lambda: None,
    "molt_typing_private_payload": _typing_private_payload,
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


_private = _load_module("_molt_private_typing", {str(STDLIB_ROOT / "_typing.py")!r})

rows = [
    (name, type(getattr(_private, name)).__name__, bool(callable(getattr(_private, name))))
    for name in sorted(dir(_private))
    if not name.startswith("_") and name != "annotations"
]
for name, type_name, is_callable in rows:
    print(f"ROW|{{name}}|{{type_name}}|{{is_callable}}")

checks = {{
    "behavior": (
        _private.Generic is Generic
        and _private.ParamSpec is ParamSpec
        and _private.ParamSpecArgs is ParamSpecArgs
        and _private.ParamSpecKwargs is ParamSpecKwargs
        and _private.TypeAliasType is TypeAliasType
        and _private.TypeVar is TypeVar
        and _private.TypeVarTuple is TypeVarTuple
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


def test__typing_public_surface_matches_expected_shape() -> None:
    rows, checks = _run_probe()
    assert rows == [
        ("Generic", "type", "True"),
        ("ParamSpec", "type", "True"),
        ("ParamSpecArgs", "type", "True"),
        ("ParamSpecKwargs", "type", "True"),
        ("TypeAliasType", "type", "True"),
        ("TypeVar", "type", "True"),
        ("TypeVarTuple", "type", "True"),
    ]
    assert checks == {"behavior": "True"}
