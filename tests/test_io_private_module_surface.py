from __future__ import annotations

import sys
from pathlib import Path

import pytest

from tests.surface_process_guard import run_surface_test_process


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"

_PROBE = f"""
import abc
import importlib.util
import _io as _host_native_io
import sys
import types
from tests.stdlib_intrinsic_registry import install_registry

_published_open = _host_native_io.open
_intrinsic_requests = []
_promotions = []

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


_resolver = install_registry({{
    "molt_function_set_builtin": lambda function: _promotions.append((function.__name__, function.__text_signature__)),
    "molt_io_class": _io_class,
    "molt_io_default_buffer_size": lambda: _host_native_io.DEFAULT_BUFFER_SIZE,
    "molt_builtin_class_lookup": _builtin_class_lookup,
}})
_resolve = _resolver.require_intrinsic


def _recording_require_intrinsic(name, namespace=None):
    _intrinsic_requests.append(name)
    return _resolve(name, namespace)


_resolver.require_intrinsic = _recording_require_intrinsic


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
        and _private.DEFAULT_BUFFER_SIZE == _host_native_io.DEFAULT_BUFFER_SIZE
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
    == ["molt_io_class", "molt_builtin_class_lookup", "molt_io_default_buffer_size", "molt_function_set_builtin"] * 2,
    "promotion": _promotions == [("text_encoding", "($module, encoding, stacklevel=2, /)")] * 2,
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
        [sys.executable, "-B", "-c", _PROBE],
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
        # The host double records promotion; actual builtin kind is qualified
        # by the compiled logging_handlers_rotation differential fixture.
        ("text_encoding", "function", "True"),
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
        "promotion": "True",
        "unsupported_operation": "True",
    }


@pytest.mark.parametrize("buffer_size", [8192, 131072])
def test_text_encoding_public_contract_and_warning_frames(monkeypatch, buffer_size):
    import _io as native_io
    import importlib.util
    import warnings
    from types import SimpleNamespace

    from tests.stdlib_intrinsic_registry import intrinsic_registry

    promotions = []
    hooks = {
        "molt_io_class": lambda name: getattr(native_io, name),
        "molt_io_default_buffer_size": lambda: buffer_size,
        "molt_builtin_class_lookup": lambda name: getattr(native_io, name),
        "molt_function_set_builtin": lambda function: promotions.append(function),
    }
    with intrinsic_registry(hooks):
        spec = importlib.util.spec_from_file_location(
            "_molt_io_text", STDLIB_ROOT / "_io.py"
        )
        private = importlib.util.module_from_spec(spec)
        private.open = native_io.open  # Existing native publication boundary.
        spec.loader.exec_module(private)
    assert promotions == [private.text_encoding]
    assert private.DEFAULT_BUFFER_SIZE == buffer_size
    assert (
        private.text_encoding.__text_signature__
        == "($module, encoding, stacklevel=2, /)"
    )

    with monkeypatch.context() as patch, intrinsic_registry(hooks):
        patch.setitem(sys.modules, "_io", private)
        patch.setattr(private, "DEFAULT_BUFFER_SIZE", -123)
        for name in ("_pyio_text", "_pyio_text_context"):
            patch.delitem(sys.modules, name, raising=False)
        spec = importlib.util.spec_from_file_location(
            "_molt_pyio_text", STDLIB_ROOT / "_pyio.py"
        )
        pure = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(pure)
        assert pure.DEFAULT_BUFFER_SIZE == buffer_size
        assert private.DEFAULT_BUFFER_SIZE == -123

    assert pure.DEFAULT_BUFFER_SIZE == buffer_size

    sentinel = object()
    for utf8_mode, expected in ((0, "locale"), (1, "utf-8")):
        flags = SimpleNamespace(utf8_mode=utf8_mode, warn_default_encoding=0)
        monkeypatch.setattr(private, "_sys", SimpleNamespace(flags=flags))
        monkeypatch.setattr(pure, "sys", SimpleNamespace(flags=flags))
        assert private.text_encoding(None) == pure.text_encoding(None) == expected
        assert private.text_encoding(sentinel) is sentinel
        assert pure.text_encoding(encoding=sentinel, stacklevel=object()) is sentinel
    with pytest.raises(TypeError):
        private.text_encoding(encoding=None)
    for value, error in (
        (1.0, TypeError),
        (2**31, OverflowError),
        (-(2**31) - 1, OverflowError),
    ):
        with pytest.raises(error):
            private.text_encoding("explicit", value)

    class StackLevel:
        def __index__(self):
            return 1

    assert private.text_encoding("explicit", StackLevel()) == "explicit"

    flags = SimpleNamespace(utf8_mode=0, warn_default_encoding=1)
    monkeypatch.setattr(private, "_sys", SimpleNamespace(flags=flags))
    monkeypatch.setattr(pure, "sys", SimpleNamespace(flags=flags))
    for function in (private.text_encoding, pure.text_encoding):
        for level in (-1, 0, 1):
            with warnings.catch_warnings(record=True) as caught:
                warnings.simplefilter("always")
                function(None, level)
            assert len(caught) == 1
            warning = caught[0]
            assert warning.category is EncodingWarning
            assert str(warning.message) == "'encoding' argument not specified."
            expected_path = (
                STDLIB_ROOT / "_pyio.py"
                if function is pure.text_encoding and level <= 0
                else Path(__file__)
            )
            assert Path(warning.filename) == expected_path
