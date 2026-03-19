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


def _generator():
    yield 1


async def _coroutine():
    return 1


async def _async_generator():
    yield 1


builtins._molt_intrinsics = {{
    "molt_abc_bootstrap": lambda: None,
    "molt_collections_abc_runtime_types": lambda: {{
        "bytes_iterator": type(iter(b"")),
        "bytearray_iterator": type(iter(bytearray())),
        "dict_keyiterator": type(iter({{}}.keys())),
        "dict_valueiterator": type(iter({{}}.values())),
        "dict_itemiterator": type(iter({{}}.items())),
        "list_iterator": type(iter([])),
        "list_reverseiterator": type(reversed([])),
        "range_iterator": type(iter(range(0))),
        "longrange_iterator": type(iter(range(1 << 65, (1 << 65) + 1))),
        "set_iterator": type(iter(set())),
        "str_iterator": type(iter("")),
        "tuple_iterator": type(iter(())),
        "zip_iterator": type(iter(zip())),
        "dict_keys": type({{}}.keys()),
        "dict_values": type({{}}.values()),
        "dict_items": type({{}}.items()),
        "mappingproxy": type(type.__dict__),
        "framelocalsproxy": type((lambda: None).__globals__),
        "generator": type(_generator()),
        "coroutine": type(_coroutine()),
        "async_generator": type(_async_generator()),
    }},
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


_private = _load_module("_molt_private_collections_abc", {str(STDLIB_ROOT / "_collections_abc.py")!r})

rows = [
    (name, type(getattr(_private, name)).__name__, bool(callable(getattr(_private, name))))
    for name in sorted(dir(_private))
    if not name.startswith("_") and name != "annotations"
]
for name, type_name, is_callable in rows:
    print(f"ROW|{{name}}|{{type_name}}|{{is_callable}}")

checks = {{
    "behavior": (
        hasattr(_private, "Iterable")
        and hasattr(_private, "Mapping")
        and _private.__name__ == "collections.abc"
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


def test__collections_abc_public_surface_matches_expected_shape() -> None:
    rows, checks = _run_probe()
    names = [name for name, _, _ in rows]
    assert "molt_abc_bootstrap" not in names
    assert "molt_collections_abc_runtime_types" not in names
    assert "Iterable" in names
    assert "Mapping" in names
    assert "MutableSequence" in names
    assert checks == {"behavior": "True"}
