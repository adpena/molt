from __future__ import annotations

import sys
from pathlib import Path

from tests.surface_process_guard import run_surface_test_process


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"

_PROBE = f"""
import builtins
import importlib.util
import struct
import sys
import types


def _coerce_value(_ctype, value):
    raw = getattr(value, "value", value)
    kind = getattr(_ctype, "_kind", "int")
    bits = int(getattr(_ctype, "_bits", getattr(_ctype, "_size", 8) * 8))
    signed = bool(getattr(_ctype, "_signed", True))
    if kind == "bool":
        return bool(raw)
    if kind == "char":
        if isinstance(raw, int):
            if raw < 0 or raw > 255:
                raise TypeError("one character bytes, bytearray, or an integer in range(256) expected")
            return bytes([raw])
        if isinstance(raw, (bytes, bytearray)) and len(raw) == 1:
            return bytes(raw)
        raise TypeError("one character bytes, bytearray, or an integer in range(256) expected")
    if kind == "float":
        val = float(raw)
        return struct.unpack("f", struct.pack("f", val))[0] if bits == 32 else val
    if kind == "void_p" and raw is None:
        return None
    raw_int = int(raw)
    modulo = 1 << bits
    wrapped = raw_int % modulo
    if signed and wrapped >= (1 << (bits - 1)):
        wrapped -= modulo
    return wrapped


def _default_value(ctype):
    kind = getattr(ctype, "_kind", "int")
    if kind == "bool":
        return False
    if kind == "char":
        return b"\\x00"
    if kind == "float":
        return 0.0
    if kind == "void_p":
        return None
    return 0


def _sizeof(obj_or_type):
    size = getattr(obj_or_type, "_size", None)
    if isinstance(size, int):
        return size
    return getattr(type(obj_or_type), "_size", 0)


builtins._molt_intrinsics = {{
    "molt_ctypes_require_ffi": lambda: None,
    "molt_ctypes_coerce_value": _coerce_value,
    "molt_ctypes_default_value": _default_value,
    "molt_ctypes_sizeof": _sizeof,
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


_load_module("ctypes", {str(STDLIB_ROOT / "ctypes" / "__init__.py")!r})
_private = _load_module("_ctypes", {str(STDLIB_ROOT / "_ctypes.py")!r})

rows = [
    (name, type(value).__name__, bool(callable(value)))
    for name, value in sorted(_private.__dict__.items())
    if not name.startswith("_")
]
for name, type_name, is_callable in rows:
    print(f"ROW|{{name}}|{{type_name}}|{{is_callable}}")


class Pair(_private.Structure):
    _fields_ = [("left", _private.c_int), ("right", _private.c_int)]


value = _private.c_int(7)
pair = Pair(3, 4)
ptr = _private.pointer(value)

checks = {{
    "scalar": int(value) == 7 and _private.sizeof(_private.c_int) == 4,
    "signed_wrap": _private.c_int8(255).value == -1,
    "unsigned_wrap": _private.c_uint8(-1).value == 255,
    "uint64": _private.c_uint64(-1).value == 18446744073709551615,
    "char": _private.c_char(65).value == b"A" and _private.c_char(b"z").value == b"z",
    "float": _private.c_float(0.1).value == struct.unpack("f", struct.pack("f", 0.1))[0],
    "structure": pair.left == 3 and pair.right == 4 and _private.sizeof(Pair) == 8,
    "pointer": ptr.contents is value,
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


def test__ctypes_public_surface_matches_expected_shape() -> None:
    rows, checks = _run_probe()
    assert rows == [
        ("Array", "type", "True"),
        ("Structure", "_StructureMeta", "True"),
        ("c_bool", "_CTypeSpec", "True"),
        ("c_byte", "_CTypeSpec", "True"),
        ("c_char", "_CTypeSpec", "True"),
        ("c_double", "_CTypeSpec", "True"),
        ("c_float", "_CTypeSpec", "True"),
        ("c_int", "_CTypeSpec", "True"),
        ("c_int16", "_CTypeSpec", "True"),
        ("c_int32", "_CTypeSpec", "True"),
        ("c_int64", "_CTypeSpec", "True"),
        ("c_int8", "_CTypeSpec", "True"),
        ("c_long", "_CTypeSpec", "True"),
        ("c_longlong", "_CTypeSpec", "True"),
        ("c_short", "_CTypeSpec", "True"),
        ("c_size_t", "_CTypeSpec", "True"),
        ("c_ubyte", "_CTypeSpec", "True"),
        ("c_uint", "_CTypeSpec", "True"),
        ("c_uint16", "_CTypeSpec", "True"),
        ("c_uint32", "_CTypeSpec", "True"),
        ("c_uint64", "_CTypeSpec", "True"),
        ("c_uint8", "_CTypeSpec", "True"),
        ("c_ulong", "_CTypeSpec", "True"),
        ("c_ulonglong", "_CTypeSpec", "True"),
        ("c_ushort", "_CTypeSpec", "True"),
        ("c_void_p", "_CTypeSpec", "True"),
        ("pointer", "function", "True"),
        ("sizeof", "function", "True"),
    ]
    assert checks == {
        "char": "True",
        "float": "True",
        "pointer": "True",
        "scalar": "True",
        "signed_wrap": "True",
        "structure": "True",
        "uint64": "True",
        "unsigned_wrap": "True",
    }
