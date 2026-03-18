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
import types as _host_types
import types


def _bootstrap():
    fallback_types = {{
        "CapsuleType": type("CapsuleType", (), {{}}),
    }}
    keys = (
        "AsyncGeneratorType",
        "BuiltinFunctionType",
        "BuiltinMethodType",
        "CapsuleType",
        "CellType",
        "ClassMethodDescriptorType",
        "CodeType",
        "CoroutineType",
        "EllipsisType",
        "FrameType",
        "FunctionType",
        "GeneratorType",
        "GenericAlias",
        "GetSetDescriptorType",
        "LambdaType",
        "MappingProxyType",
        "MemberDescriptorType",
        "MethodDescriptorType",
        "MethodType",
        "MethodWrapperType",
        "ModuleType",
        "NoneType",
        "NotImplementedType",
        "SimpleNamespace",
        "TracebackType",
        "UnionType",
        "WrapperDescriptorType",
        "DynamicClassAttribute",
        "coroutine",
        "get_original_bases",
        "new_class",
        "prepare_class",
        "resolve_bases",
    )
    data = {{}}
    for name in keys:
        if hasattr(_host_types, name):
            data[name] = getattr(_host_types, name)
        else:
            data[name] = fallback_types[name]
    return data


builtins._molt_intrinsics = {{
    "molt_types_bootstrap": _bootstrap,
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


_load_module("types", {str(STDLIB_ROOT / "types.py")!r})
mod = _load_module("_types", {str(STDLIB_ROOT / "_types.py")!r})

rows = [
    (name, type(value).__name__, bool(callable(value)))
    for name, value in sorted(mod.__dict__.items())
    if not name.startswith("_")
]
for name, type_name, is_callable in rows:
    print(f"ROW|{{name}}|{{type_name}}|{{is_callable}}")

checks = {{
    "shape": (
        mod.FunctionType is _host_types.FunctionType
        and mod.ModuleType is _host_types.ModuleType
        and mod.SimpleNamespace is _host_types.SimpleNamespace
        and mod.UnionType is _host_types.UnionType
        and mod.GenericAlias is _host_types.GenericAlias
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


def test__types_public_surface_matches_expected_shape() -> None:
    rows, checks = _run_probe()
    assert rows == [
        ("AsyncGeneratorType", "type", "True"),
        ("BuiltinFunctionType", "type", "True"),
        ("BuiltinMethodType", "type", "True"),
        ("CapsuleType", "type", "True"),
        ("CellType", "type", "True"),
        ("ClassMethodDescriptorType", "type", "True"),
        ("CodeType", "type", "True"),
        ("CoroutineType", "type", "True"),
        ("EllipsisType", "type", "True"),
        ("FrameType", "type", "True"),
        ("FunctionType", "type", "True"),
        ("GeneratorType", "type", "True"),
        ("GenericAlias", "type", "True"),
        ("GetSetDescriptorType", "type", "True"),
        ("LambdaType", "type", "True"),
        ("MappingProxyType", "type", "True"),
        ("MemberDescriptorType", "type", "True"),
        ("MethodDescriptorType", "type", "True"),
        ("MethodType", "type", "True"),
        ("MethodWrapperType", "type", "True"),
        ("ModuleType", "type", "True"),
        ("NoneType", "type", "True"),
        ("NotImplementedType", "type", "True"),
        ("SimpleNamespace", "type", "True"),
        ("TracebackType", "type", "True"),
        ("UnionType", "type", "True"),
        ("WrapperDescriptorType", "type", "True"),
    ]
    assert checks == {"shape": "True"}
