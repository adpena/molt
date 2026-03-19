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


def _heapify(heap):
    heap.sort()


def _heappush(heap, item):
    heap.append(item)
    heap.sort()


def _heappop(heap):
    if not heap:
        raise IndexError("index out of range")
    return heap.pop(0)


def _heapreplace(heap, item):
    if not heap:
        raise IndexError("index out of range")
    out = heap[0]
    heap[0] = item
    heap.sort()
    return out


def _heappushpop(heap, item):
    if heap and heap[0] < item:
        out = heap[0]
        heap[0] = item
        heap.sort()
        return out
    return item


builtins._molt_intrinsics = {{
    "molt_heapq_heapify": _heapify,
    "molt_heapq_heappush": _heappush,
    "molt_heapq_heappop": _heappop,
    "molt_heapq_heapreplace": _heapreplace,
    "molt_heapq_heappushpop": _heappushpop,
    "molt_heapq_heapify_max": lambda heap: heap.sort(reverse=True),
    "molt_heapq_heappop_max": lambda heap: heap.pop(0),
    "molt_heapq_nsmallest": lambda n, iterable, key=None: sorted(iterable, key=key)[:n],
    "molt_heapq_nlargest": lambda n, iterable, key=None: sorted(iterable, key=key, reverse=True)[:n],
    "molt_heapq_merge": lambda iterables, key, reverse: sorted([item for iterable in iterables for item in iterable], key=key, reverse=reverse),
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


_load_module("heapq", {str(STDLIB_ROOT / "heapq.py")!r})
_private = _load_module("_heapq", {str(STDLIB_ROOT / "_heapq.py")!r})

rows = [
    (name, type(value).__name__, bool(callable(value)))
    for name, value in sorted(_private.__dict__.items())
    if not name.startswith("_")
]
for name, type_name, is_callable in rows:
    print(f"ROW|{{name}}|{{type_name}}|{{is_callable}}")

heap = [5, 1, 3]
_private.heapify(heap)
_private.heappush(heap, 2)
checks = {{
    "behavior": (
        heap == [1, 2, 3, 5]
        and _private.heappop(heap) == 1
        and heap == [2, 3, 5]
        and _private.heapreplace(heap, 4) == 2
        and heap == [3, 4, 5]
        and _private.heappushpop(heap, 1) == 1
        and heap == [3, 4, 5]
        and _private.heappushpop(heap, 6) == 3
        and heap == [4, 5, 6]
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


def test__heapq_public_surface_matches_expected_shape() -> None:
    rows, checks = _run_probe()
    assert rows == [
        ("heapify", "function", "True"),
        ("heappop", "function", "True"),
        ("heappush", "function", "True"),
        ("heappushpop", "function", "True"),
        ("heapreplace", "function", "True"),
    ]
    assert checks == {"behavior": "True"}
