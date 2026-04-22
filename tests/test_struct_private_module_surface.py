from __future__ import annotations

import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"

_PROBE = f"""
import builtins
import importlib.util
import struct as _host_struct
import sys
import types

builtins._molt_intrinsics = {{
    "molt_struct_pack": lambda fmt, values: _host_struct.pack(fmt, *values),
    "molt_struct_unpack": _host_struct.unpack,
    "molt_struct_calcsize": _host_struct.calcsize,
    "molt_struct_pack_into": lambda buffer, offset, data: buffer.__setitem__(slice(offset, offset + len(data)), data),
    "molt_struct_unpack_from": _host_struct.unpack_from,
    "molt_struct_iter_unpack": lambda fmt, buffer: tuple(_host_struct.iter_unpack(fmt, buffer)),
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


_load_module("struct", {str(STDLIB_ROOT / "struct.py")!r})
_private = _load_module("_struct", {str(STDLIB_ROOT / "_struct.py")!r})

rows = [
    (name, type(value).__name__, bool(callable(value)))
    for name, value in sorted(_private.__dict__.items())
    if not name.startswith("_")
]
for name, type_name, is_callable in rows:
    print(f"ROW|{{name}}|{{type_name}}|{{is_callable}}")

buf = bytearray(4)
_private.pack_into(">H", buf, 0, 0x1234)
checks = {{
    "behavior": (
        _private.calcsize(">H") == 2
        and _private.pack(">H", 0x1234) == b"\\x12\\x34"
        and _private.unpack(">H", b"\\x12\\x34") == (0x1234,)
        and _private.unpack_from(">H", b"\\x12\\x34", 0) == (0x1234,)
        and tuple(_private.iter_unpack(">H", b"\\x12\\x34\\x56\\x78")) == ((0x1234,), (0x5678,))
        and bytes(buf[:2]) == b"\\x12\\x34"
        and _private.Struct(">H").pack(0x1234) == b"\\x12\\x34"
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


def test__struct_public_surface_matches_expected_shape() -> None:
    rows, checks = _run_probe()
    expected_names = [
        "Struct",
        "calcsize",
        "error",
        "iter_unpack",
        "pack",
        "pack_into",
        "unpack",
        "unpack_from",
    ]
    actual_names = [name for name, _, _ in rows]
    assert actual_names == expected_names
    # All non-type entries must be callable
    for name, type_name, is_callable in rows:
        if type_name != "type":
            assert is_callable == "True", f"{name} should be callable"
    assert checks == {"behavior": "True"}
