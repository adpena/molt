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


class _CodecInfo:
    def __init__(self, name):
        self.name = name


_fake_codecs = types.ModuleType("codecs")
_fake_codecs.lookup = lambda encoding: _CodecInfo(encoding)
sys.modules["codecs"] = _fake_codecs


def _encode(obj, encoding, errors):
    return f"enc:{{encoding}}:{{errors}}:{{obj}}"


def _decode(obj, encoding, errors):
    return f"dec:{{encoding}}:{{errors}}:{{obj}}"


def _lookup_name(name):
    if name == "utf8":
        return "utf-8"
    return name


builtins._molt_intrinsics = {{
    "molt_codecs_decode": _decode,
    "molt_codecs_encode": _encode,
    "molt_codecs_lookup_name": _lookup_name,
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


_private = _load_module("_molt_private_codecs", {str(STDLIB_ROOT / "_codecs.py")!r})

rows = [
    (name, type(getattr(_private, name)).__name__, bool(callable(getattr(_private, name))))
    for name in sorted(dir(_private))
    if not name.startswith("_") and name != "annotations"
]
for name, type_name, is_callable in rows:
    print(f"ROW|{{name}}|{{type_name}}|{{is_callable}}")

codec = _private.getcodec("utf8")
checks = {{
    "behavior": (
        _private.encode("x", "utf-8", "strict") == "enc:utf-8:strict:x"
        and _private.decode("y", "utf-8", "ignore") == "dec:utf-8:ignore:y"
        and codec.encode("z") == ("enc:utf-8:strict:z", 1)
        and codec.decode("w", "replace") == ("dec:utf-8:replace:w", 1)
        and _private.lookup("utf-8").name == "utf-8"
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


def test__codecs_public_surface_matches_expected_shape() -> None:
    rows, checks = _run_probe()
    assert rows == [
        ("decode", "function", "True"),
        ("encode", "function", "True"),
        ("getcodec", "function", "True"),
        ("lookup", "function", "True"),
    ]
    assert checks == {"behavior": "True"}
