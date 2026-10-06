from __future__ import annotations

import sys
from pathlib import Path

from tests.surface_process_guard import run_surface_test_process


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"

_PROBE = f"""
import abc
import builtins
import importlib.util
import _io as _host_native_io
import sys
import types

_published_open = _host_native_io.open
_intrinsic_requests = []

# Capture code before replacing the host loader's _io namespace. The unseeded
# provider intentionally has no open; importing our fixture must not require
# file I/O through that namespace or accidentally borrow host builtins.open.
_code = {{}}
for _path in ({str(STDLIB_ROOT / "_io.py")!r}, {str(STDLIB_ROOT / "io.py")!r}):
    with _published_open(_path, "rb") as _source:
        _code[_path] = compile(_source.read(), _path, "exec")


def _io_class(name):
    return getattr(_host_native_io, name)


def _builtin_class_lookup(name):
    # The runtime resolves builtin exception classes by name; the host's _io
    # owns the same UnsupportedOperation identity.
    return getattr(_host_native_io, name)


builtins._molt_intrinsics = {{
    "molt_io_class": _io_class,
    "molt_builtin_class_lookup": _builtin_class_lookup,
}}

_intrinsics_mod = types.ModuleType("_intrinsics")


def _require_intrinsic(name, namespace=None):
    _intrinsic_requests.append(name)
    intrinsics = getattr(builtins, "_molt_intrinsics", {{}})
    if name in intrinsics:
        value = intrinsics[name]
        if namespace is not None:
            namespace[name] = value
        return value
    raise RuntimeError(f"intrinsic unavailable: {{name}}")


_intrinsics_mod.require_intrinsic = _require_intrinsic
sys.modules["_intrinsics"] = _intrinsics_mod


def _load_module(name, path_text, *, published_open=None):
    spec = importlib.util.spec_from_file_location(name, path_text)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    if published_open is not None:
        module.open = published_open
    sys.modules[name] = module
    exec(_code[path_text], module.__dict__)
    return module


# The provider must initialize without importing the public facade. Its native
# callable is supplied by publication, not by this Python module's body.
sys.modules["io"] = types.ModuleType("io")
_private = _load_module(
    "_io", {str(STDLIB_ROOT / "_io.py")!r}, published_open=_published_open
)
_public = _load_module("io", {str(STDLIB_ROOT / "io.py")!r})

rows = [
    (name, type(value).__name__, bool(callable(value)))
    for name, value in sorted(_private.__dict__.items())
    if not name.startswith("_")
]
for name, type_name, is_callable in rows:
    print(f"ROW|{{name}}|{{type_name}}|{{is_callable}}")

# A separate provider publication can intentionally exclude open. Importing
# either facade must preserve memory I/O without borrowing host builtins.open.
_without_open = _load_module("_io", {str(STDLIB_ROOT / "_io.py")!r})
_without_open_public = _load_module("io", {str(STDLIB_ROOT / "io.py")!r})

try:
    _private.open(0, "invalid")
except ValueError:
    _native_mode_error = True
else:
    _native_mode_error = False

checks = {{
    "constants": (
        _private.SEEK_SET == 0
        and _private.SEEK_CUR == 1
        and _private.SEEK_END == 2
        and _private.DEFAULT_BUFFER_SIZE == 8192
    ),
    "classes": (
        _private._IOBase is _host_native_io._IOBase
        and _private._RawIOBase is _host_native_io._RawIOBase
        and _private._BufferedIOBase is _host_native_io._BufferedIOBase
        and _private._TextIOBase is _host_native_io._TextIOBase
        and _private.FileIO is _host_native_io.FileIO
        and _private.BytesIO is _host_native_io.BytesIO
        and _private.StringIO is _host_native_io.StringIO
    ),
    "aliases": all(
        getattr(_public, name) is getattr(_private, name)
        for name in _private.__all__
    ),
    # CPython layering: _io owns the concrete bases, io publishes ABCs over them.
    "public_abcs": (
        all(
            type(getattr(_public, name)) is abc.ABCMeta
            and getattr(_private, "_" + name) in getattr(_public, name).__mro__
            and name not in vars(_private)
            for name in ("IOBase", "RawIOBase", "BufferedIOBase", "TextIOBase")
        )
        and issubclass(_public.FileIO, _public.RawIOBase)
        and issubclass(_public.BytesIO, _public.BufferedIOBase)
        and issubclass(_public.StringIO, _public.TextIOBase)
    ),
    "open": _private.open is _published_open and _public.open is _published_open,
    "native_mode_error": _native_mode_error,
    "excluded_open": all(
        "open" not in vars(module) and "open" not in module.__all__
        for module in (_without_open, _without_open_public)
    ),
    "memory_io_without_open": (
        _without_open_public.BytesIO is _without_open.BytesIO
        and _without_open_public.BytesIO(b"memory").read() == b"memory"
        and _without_open_public.StringIO("memory").read() == "memory"
    ),
    "intrinsics": _intrinsic_requests
    == ["molt_io_class", "molt_builtin_class_lookup"] * 2,
    "unsupported_operation": (
        _private.UnsupportedOperation.__module__ == "io"
        and issubclass(_private.UnsupportedOperation, OSError)
        and issubclass(_private.UnsupportedOperation, ValueError)
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
        check=False,
    )
    assert proc.returncode == 0, proc.stderr
    rows: list[tuple[str, str, str]] = []
    checks: dict[str, str] = {}
    for line in proc.stdout.splitlines():
        prefix, *rest = line.split("|")
        if prefix == "ROW":
            rows.append((rest[0], rest[1], rest[2]))
        elif prefix == "CHECK":
            checks[rest[0]] = rest[1]
    return rows, checks


def test__io_public_surface_matches_expected_shape() -> None:
    rows, checks = _run_probe()
    assert rows == [
        ("BufferedRandom", "type", "True"),
        ("BufferedReader", "type", "True"),
        ("BufferedWriter", "type", "True"),
        ("BytesIO", "type", "True"),
        ("DEFAULT_BUFFER_SIZE", "int", "False"),
        ("FileIO", "type", "True"),
        ("SEEK_CUR", "int", "False"),
        ("SEEK_END", "int", "False"),
        ("SEEK_SET", "int", "False"),
        ("StringIO", "type", "True"),
        ("TextIOWrapper", "type", "True"),
        ("UnsupportedOperation", "type", "True"),
        ("open", "builtin_function_or_method", "True"),
    ]
    assert checks == {
        "aliases": "True",
        "classes": "True",
        "constants": "True",
        "excluded_open": "True",
        "intrinsics": "True",
        "memory_io_without_open": "True",
        "native_mode_error": "True",
        "open": "True",
        "public_abcs": "True",
        "unsupported_operation": "True",
    }
