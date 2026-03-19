from __future__ import annotations

import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"

_PROBE = f"""
import builtins
import importlib.util
import pickle as _host_pickle
import sys
import types


builtins._molt_intrinsics = {{
    "molt_stdlib_probe": lambda: None,
    "molt_pickle_dumps_core": lambda obj, protocol, fix_imports, _persistent_id, buffer_callback, _dispatch_table: _host_pickle.dumps(
        obj,
        protocol=protocol,
        fix_imports=fix_imports,
        buffer_callback=buffer_callback,
    ),
    "molt_pickle_loads_core": lambda data, fix_imports, encoding, errors, _persistent_load, _buffers_iter, buffers: _host_pickle.loads(
        data,
        fix_imports=fix_imports,
        encoding=encoding,
        errors=errors,
        buffers=buffers,
    ),
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


_load_module("pickle", {str(STDLIB_ROOT / "pickle.py")!r})
_private = _load_module("_pickle", {str(STDLIB_ROOT / "_pickle.py")!r})

rows = [
    (name, type(getattr(_private, name)).__name__, bool(callable(getattr(_private, name))))
    for name in sorted(dir(_private))
    if not name.startswith("_") and name != "annotations"
]
for name, type_name, is_callable in rows:
    print(f"ROW|{{name}}|{{type_name}}|{{is_callable}}")

payload = {{"answer": 42}}
blob = _private.dumps(payload)
checks = {{
    "behavior": _private.loads(blob) == payload,
    "protocols": (
        isinstance(_private.DEFAULT_PROTOCOL, int)
        and isinstance(_private.HIGHEST_PROTOCOL, int)
        and _private.HIGHEST_PROTOCOL >= _private.DEFAULT_PROTOCOL
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


def test__pickle_public_surface_matches_expected_shape() -> None:
    rows, checks = _run_probe()
    assert rows == [
        ("DEFAULT_PROTOCOL", "int", "False"),
        ("HIGHEST_PROTOCOL", "int", "False"),
        ("PickleBuffer", "type", "True"),
        ("PickleError", "type", "True"),
        ("Pickler", "type", "True"),
        ("PicklingError", "type", "True"),
        ("Unpickler", "type", "True"),
        ("UnpicklingError", "type", "True"),
        ("dump", "function", "True"),
        ("dumps", "function", "True"),
        ("load", "function", "True"),
        ("loads", "function", "True"),
    ]
    assert checks == {"behavior": "True", "protocols": "True"}
