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


class _State:
    next_handle = 0
    store = {{}}


def _new():
    _State.next_handle += 1
    handle = _State.next_handle
    _State.store[handle] = []
    return handle


def _from_pairs(pairs):
    _State.next_handle += 1
    handle = _State.next_handle
    _State.store[handle] = list(pairs)
    return handle


def _setitem(handle, key, value):
    items = _State.store[handle]
    for idx, (k, _v) in enumerate(items):
        if k == key:
            items[idx] = (key, value)
            return
    items.append((key, value))


def _getitem(handle, key):
    for k, v in _State.store[handle]:
        if k == key:
            return v
    raise KeyError(key)


def _delitem(handle, key):
    items = _State.store[handle]
    for idx, (k, _v) in enumerate(items):
        if k == key:
            items.pop(idx)
            return
    raise KeyError(key)


builtins._molt_intrinsics = {{
    "molt_ordereddict_new": _new,
    "molt_ordereddict_from_pairs": _from_pairs,
    "molt_ordereddict_setitem": _setitem,
    "molt_ordereddict_getitem": _getitem,
    "molt_ordereddict_delitem": _delitem,
    "molt_ordereddict_contains": lambda handle, key: any(k == key for k, _ in _State.store[handle]),
    "molt_ordereddict_len": lambda handle: len(_State.store[handle]),
    "molt_ordereddict_keys": lambda handle: [k for k, _ in _State.store[handle]],
    "molt_ordereddict_values": lambda handle: [v for _, v in _State.store[handle]],
    "molt_ordereddict_items": lambda handle: list(_State.store[handle]),
    "molt_ordereddict_move_to_end": lambda handle, key, last: None,
    "molt_ordereddict_pop": lambda handle, key, default=None: default,
    "molt_ordereddict_popitem": lambda handle, last=True: _State.store[handle].pop(-1 if last else 0),
    "molt_ordereddict_update": lambda handle, other: None,
    "molt_ordereddict_clear": lambda handle: _State.store[handle].clear(),
    "molt_ordereddict_copy": lambda handle: _from_pairs(_State.store[handle]),
    "molt_ordereddict_drop": lambda handle: _State.store.pop(handle, None),
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


_private = _load_module("_molt_private_collections", {str(STDLIB_ROOT / "_collections.py")!r})

rows = [
    (name, type(getattr(_private, name)).__name__, bool(callable(getattr(_private, name))))
    for name in sorted(dir(_private))
    if not name.startswith("_") and name != "annotations"
]
for name, type_name, is_callable in rows:
    print(f"ROW|{{name}}|{{type_name}}|{{is_callable}}")

od = _private.OrderedDict([("a", 1), ("b", 2)])
od["c"] = 3
checks = {{
    "behavior": (
        list(od.keys()) == ["a", "b", "c"]
        and list(od.values()) == [1, 2, 3]
        and list(od.items()) == [("a", 1), ("b", 2), ("c", 3)]
        and od["b"] == 2
        and len(od) == 3
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


def test__collections_public_surface_matches_expected_shape() -> None:
    rows, checks = _run_probe()
    assert rows == [("OrderedDict", "type", "True")]
    assert checks == {"behavior": "True"}
