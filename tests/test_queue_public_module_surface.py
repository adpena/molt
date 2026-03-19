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


def _new_queue(_maxsize):
    _State.next_handle += 1
    handle = _State.next_handle
    _State.queues[handle] = []
    return handle


def _put(handle, item, _block, _timeout):
    _State.queues[handle].append(item)
    return True


def _get(handle, _block, _timeout, sentinel):
    queue = _State.queues[handle]
    if not queue:
        return sentinel
    return queue.pop(0)


builtins._molt_intrinsics = {{
    "molt_queue_new": _new_queue,
    "molt_queue_lifo_new": _new_queue,
    "molt_queue_priority_new": _new_queue,
    "molt_queue_qsize": lambda handle: len(_State.queues[handle]),
    "molt_queue_empty": lambda handle: not _State.queues[handle],
    "molt_queue_full": lambda _handle: False,
    "molt_queue_put": _put,
    "molt_queue_get": _get,
    "molt_queue_task_done": lambda _handle: True,
    "molt_queue_join": lambda _handle: None,
    "molt_queue_drop": lambda handle: _State.queues.pop(handle, None),
    "molt_module_cache_set": lambda _name, _module: None,
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

_queue_mod = types.ModuleType("_queue")

class Empty(Exception):
    pass

class SimpleQueue:
    pass

_queue_mod.Empty = Empty
_queue_mod.SimpleQueue = SimpleQueue
sys.modules["_queue"] = _queue_mod


def _load_module(name, path_text):
    spec = importlib.util.spec_from_file_location(name, path_text)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


queue = _load_module("queue", {str(STDLIB_ROOT / "queue.py")!r})
q = queue.Queue()
q.put("x")

checks = {{
    "behavior": q.get() == "x" and q.empty() is True and q.qsize() == 0,
    "private_handles_hidden": (
        "molt_queue_new" not in queue.__dict__
        and "molt_queue_put" not in queue.__dict__
        and "molt_queue_get" not in queue.__dict__
        and "molt_module_cache_set" not in queue.__dict__
    ),
}}
for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_queue_public_module_hides_raw_intrinsic_names() -> None:
    proc = subprocess.run(
        [sys.executable, "-c", _PROBE],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=True,
    )
    checks: dict[str, str] = {}
    for line in proc.stdout.splitlines():
        prefix, *rest = line.split("|")
        if prefix == "CHECK":
            checks[rest[0]] = rest[1]
    assert checks == {"behavior": "True", "private_handles_hidden": "True"}
