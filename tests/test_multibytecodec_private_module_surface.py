from __future__ import annotations

import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"

_PROBE = f"""
import builtins
import importlib.util
import io
import sys
import types


class _Codec:
    def encode(self, value, errors):
        return (f"enc:{{value}}:{{errors}}".encode(), len(value))

    def decode(self, value, errors):
        if isinstance(value, bytes):
            value = value.decode()
        return (f"dec:{{value}}:{{errors}}", len(value))


builtins._molt_intrinsics = {{
    "molt_capabilities_has": lambda name=None: True,
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


_private = _load_module("_molt_private_multibytecodec", {str(STDLIB_ROOT / "_multibytecodec.py")!r})
_private.MultibyteIncrementalEncoder.codec = _Codec()
_private.MultibyteIncrementalDecoder.codec = _Codec()
_private.MultibyteStreamReader.codec = _Codec()
_private.MultibyteStreamWriter.codec = _Codec()

rows = [
    (name, type(getattr(_private, name)).__name__, bool(callable(getattr(_private, name))))
    for name in sorted(dir(_private))
    if not name.startswith("_") and name != "annotations"
]
for name, type_name, is_callable in rows:
    print(f"ROW|{{name}}|{{type_name}}|{{is_callable}}")

reader = _private.MultibyteStreamReader(io.BytesIO(b"abc"))
writer_stream = io.BytesIO()
writer = _private.MultibyteStreamWriter(writer_stream, errors="replace")

checks = {{
    "behavior": (
        _private.MultibyteIncrementalEncoder(errors="ignore").encode("x") == b"enc:x:ignore"
        and _private.MultibyteIncrementalDecoder(errors="strict").decode(b"y") == "dec:y:strict"
        and reader.read() == "dec:abc:strict"
        and writer.write("z") == len(b"enc:z:replace")
        and writer_stream.getvalue() == b"enc:z:replace"
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


def test__multibytecodec_public_surface_matches_expected_shape() -> None:
    rows, checks = _run_probe()
    names = [name for name, _, _ in rows]
    assert "molt_capabilities_has" not in names
    assert "MultibyteIncrementalEncoder" in names
    assert "MultibyteIncrementalDecoder" in names
    assert "MultibyteStreamReader" in names
    assert "MultibyteStreamWriter" in names
    assert checks == {"behavior": "True"}
