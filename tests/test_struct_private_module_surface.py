from __future__ import annotations

import sys
from pathlib import Path

from tests.surface_process_guard import run_surface_test_process


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"

_PROBE = f"""
import importlib.util
import struct as _host_struct
import sys
from tests.stdlib_intrinsic_registry import install_registry

def _call_struct(error_type, operation, *args):
    try:
        return operation(*args)
    except _host_struct.error as exc:
        raise error_type(str(exc)) from None


def _iter_unpack(error_type, iterator_type, fmt, buffer):
    assert iterator_type is type(_host_struct.iter_unpack("B", b""))
    return _call_struct(error_type, _host_struct.iter_unpack, fmt, buffer)


install_registry({{
    "molt_struct_pack": lambda error_type, fmt, values: _call_struct(error_type, _host_struct.pack, fmt, *values),
    "molt_struct_unpack": lambda error_type, fmt, buffer: _call_struct(error_type, _host_struct.unpack, fmt, buffer),
    "molt_struct_calcsize": lambda error_type, fmt: _call_struct(error_type, _host_struct.calcsize, fmt),
    "molt_struct_pack_into": lambda error_type, fmt, buffer, offset, values: _call_struct(error_type, _host_struct.pack_into, fmt, buffer, offset, *values),
    "molt_struct_unpack_from": lambda error_type, fmt, buffer, offset: _call_struct(error_type, _host_struct.unpack_from, fmt, buffer, offset),
    "molt_struct_iter_unpack": _iter_unpack,
    "molt_struct_unpack_iterator_type": lambda: type(_host_struct.iter_unpack("B", b"")),
}})


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
probe = bytearray(b"ab")
iterator = _private.iter_unpack("B", probe)
first = next(iterator)
probe[1] = 9
second = next(iterator)
try:
    probe.append(0)
except BufferError:
    pinned_after_last = True
else:
    pinned_after_last = False
exhausted = next(iterator, None) is None
probe.append(0)
try:
    _private.pack("B", 256)
except _private.error:
    actual_error_class = True
else:
    actual_error_class = False
checks["lazy_and_error"] = (first == (97,) and second == (9,)
    and pinned_after_last and exhausted and len(probe) == 3 and actual_error_class)
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
    assert checks == {"behavior": "True", "lazy_and_error": "True"}
