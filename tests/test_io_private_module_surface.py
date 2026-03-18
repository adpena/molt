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


def _io_class(name):
    return type(name, (), {{"__module__": "io"}})


def _open_ex(file, mode, buffering, encoding, errors, newline, closefd, opener):
    return {{
        "file": file,
        "mode": mode,
        "buffering": buffering,
        "encoding": encoding,
        "errors": errors,
        "newline": newline,
        "closefd": closefd,
        "opener": opener,
    }}


builtins._molt_intrinsics = {{
    "molt_capabilities_require": lambda _cap: None,
    "molt_file_open_ex": _open_ex,
    "molt_file_read": lambda _handle, _size=None: b"",
    "molt_file_close": lambda _handle: None,
    "molt_io_class": _io_class,
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


_load_module("io", {str(STDLIB_ROOT / "io.py")!r})
_private = _load_module("_molt_private_io", {str(STDLIB_ROOT / "_io.py")!r})

rows = [
    (name, type(value).__name__, bool(callable(value)))
    for name, value in sorted(_private.__dict__.items())
    if not name.startswith("_")
]
for name, type_name, is_callable in rows:
    print(f"ROW|{{name}}|{{type_name}}|{{is_callable}}")

opened = _private.open("probe.txt", mode="rb")

checks = {{
    "constants": (
        _private.SEEK_SET == 0
        and _private.SEEK_CUR == 1
        and _private.SEEK_END == 2
        and _private.DEFAULT_BUFFER_SIZE == 8192
    ),
    "classes": (
        _private.IOBase.__name__ == "IOBase"
        and _private.RawIOBase.__name__ == "RawIOBase"
        and _private.BufferedIOBase.__name__ == "BufferedIOBase"
        and _private.TextIOBase.__name__ == "TextIOBase"
        and _private.FileIO.__name__ == "FileIO"
        and _private.BytesIO.__name__ == "BytesIO"
        and _private.StringIO.__name__ == "StringIO"
    ),
    "open": isinstance(opened, dict) and opened["file"] == "probe.txt" and opened["mode"] == "rb",
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


def test__io_public_surface_matches_expected_shape() -> None:
    rows, checks = _run_probe()
    assert rows == [
        ("BufferedIOBase", "type", "True"),
        ("BufferedRandom", "type", "True"),
        ("BufferedReader", "type", "True"),
        ("BufferedWriter", "type", "True"),
        ("BytesIO", "type", "True"),
        ("DEFAULT_BUFFER_SIZE", "int", "False"),
        ("FileIO", "type", "True"),
        ("IOBase", "type", "True"),
        ("RawIOBase", "type", "True"),
        ("SEEK_CUR", "int", "False"),
        ("SEEK_END", "int", "False"),
        ("SEEK_SET", "int", "False"),
        ("StringIO", "type", "True"),
        ("TextIOBase", "type", "True"),
        ("TextIOWrapper", "type", "True"),
        ("UnsupportedOperation", "type", "True"),
        ("open", "function", "True"),
    ]
    assert checks == {
        "classes": "True",
        "constants": "True",
        "open": "True",
    }
