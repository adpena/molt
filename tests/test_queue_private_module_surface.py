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
    queues = {{}}


def _queue_new(_maxsize):
    _State.next_handle += 1
    handle = _State.next_handle
    _State.queues[handle] = []
    return handle


def _queue_qsize(handle):
    return len(_State.queues[handle])


def _queue_empty(handle):
    return not _State.queues[handle]


def _queue_put(handle, item, _block, _timeout):
    _State.queues[handle].append(item)
    return True


def _queue_get(handle, _block, _timeout, sentinel):
    queue = _State.queues[handle]
    if not queue:
        return sentinel
    return queue.pop(0)


builtins._molt_intrinsics = {{
    "molt_queue_new": _queue_new,
    "molt_queue_qsize": _queue_qsize,
    "molt_queue_empty": _queue_empty,
    "molt_queue_put": _queue_put,
    "molt_queue_get": _queue_get,
    "molt_queue_drop": lambda handle: _State.queues.pop(handle, None),
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


_private = _load_module("_queue", {str(STDLIB_ROOT / "_queue.py")!r})

rows = [
    (name, type(getattr(_private, name)).__name__, bool(callable(getattr(_private, name))))
    for name in sorted(dir(_private))
    if not name.startswith("_") and name != "annotations"
]
for name, type_name, is_callable in rows:
    print(f"ROW|{{name}}|{{type_name}}|{{is_callable}}")

queue = _private.SimpleQueue()
queue.put("alpha")
queue.put("beta")
checks = {{
    "anchors_hidden": (
        "molt_queue_new" not in _private.__dict__
        and "molt_queue_qsize" not in _private.__dict__
        and "molt_queue_empty" not in _private.__dict__
        and "molt_queue_put" not in _private.__dict__
        and "molt_queue_get" not in _private.__dict__
        and "molt_queue_drop" not in _private.__dict__
    ),
    "behavior": (
        queue.qsize() == 2
        and queue.empty() is False
        and queue.get() == "alpha"
        and queue.get_nowait() == "beta"
    ),
}}
try:
    queue.get_nowait()
except _private.Empty:
    checks["empty"] = True
else:
    checks["empty"] = False
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


def test__queue_public_surface_matches_expected_shape() -> None:
    rows, checks = _run_probe()
    assert rows == [
        ("Empty", "type", "True"),
        ("SimpleQueue", "type", "True"),
    ]
    assert checks == {
        "anchors_hidden": "True",
        "behavior": "True",
        "empty": "True",
    }
