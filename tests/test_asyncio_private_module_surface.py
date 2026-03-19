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


class _AsyncFuture:
    def __init__(self, *args, **kwargs):
        self.args = args
        self.kwargs = kwargs


class _AsyncTask:
    def __init__(self, coro, *, loop=None, name=None, context=None):
        self.coro = coro
        self.loop = loop
        self.name = name
        self.context = context


_fake_asyncio = types.ModuleType("asyncio")
_fake_asyncio.Future = _AsyncFuture
_fake_asyncio.Task = _AsyncTask
sys.modules["asyncio"] = _fake_asyncio

_state = {{"running_loop": None, "event_loop": "event-loop", "current": "current-task"}}

builtins._molt_intrinsics = {{
    "molt_asyncio_running_loop_get": lambda: _state["running_loop"],
    "molt_asyncio_running_loop_set": lambda loop: _state.__setitem__("running_loop", loop),
    "molt_asyncio_event_loop_get": lambda: _state["event_loop"],
    "molt_asyncio_event_loop_policy_get": lambda: None,
    "molt_asyncio_task_registry_current": lambda: _state["current"],
    "molt_asyncio_task_registry_current_for_loop": lambda loop: ("for-loop", loop),
    "molt_asyncio_enter_task": lambda loop, task: _state.__setitem__("current", task),
    "molt_asyncio_leave_task": lambda loop, task: _state.__setitem__("current", None),
    "molt_asyncio_register_task": lambda task: None,
    "molt_asyncio_unregister_task": lambda task: None,
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


_private = _load_module("_molt_private_asyncio", {str(STDLIB_ROOT / "_asyncio.py")!r})

rows = [
    (name, type(getattr(_private, name)).__name__, bool(callable(getattr(_private, name))))
    for name in sorted(dir(_private))
    if not name.startswith("_") and name != "annotations"
]
for name, type_name, is_callable in rows:
    print(f"ROW|{{name}}|{{type_name}}|{{is_callable}}")

_private._set_running_loop("loop-a")
_private._enter_task("loop-a", "task-a")
checks = {{
    "behavior": (
        _private._get_running_loop() == "loop-a"
        and _private.get_running_loop() == "loop-a"
        and _private.get_event_loop() == "event-loop"
        and _private.current_task() == "task-a"
        and _private.current_task("loop-a") == ("for-loop", "loop-a")
        and isinstance(_private.Future(), _AsyncFuture)
        and isinstance(_private.Task("coro", loop="loop-a"), _AsyncTask)
    ),
    "private_handles_hidden": (
        "_MOLT_ASYNCIO_RUNNING_LOOP_GET" not in _private.__dict__
        and "_MOLT_ASYNCIO_RUNNING_LOOP_SET" not in _private.__dict__
        and "_MOLT_ASYNCIO_EVENT_LOOP_GET" not in _private.__dict__
        and "_MOLT_ASYNCIO_EVENT_LOOP_POLICY_GET" not in _private.__dict__
        and "_MOLT_ASYNCIO_TASK_REGISTRY_CURRENT" not in _private.__dict__
        and "_MOLT_ASYNCIO_TASK_REGISTRY_CURRENT_FOR_LOOP" not in _private.__dict__
        and "_MOLT_ASYNCIO_ENTER_TASK" not in _private.__dict__
        and "_MOLT_ASYNCIO_LEAVE_TASK" not in _private.__dict__
        and "_MOLT_ASYNCIO_REGISTER_TASK" not in _private.__dict__
        and "_MOLT_ASYNCIO_UNREGISTER_TASK" not in _private.__dict__
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


def test__asyncio_public_surface_matches_expected_shape() -> None:
    rows, checks = _run_probe()
    names = [name for name, _, _ in rows]
    assert "molt_asyncio_enter_task" not in names
    assert "Future" in names
    assert "Task" in names
    assert "current_task" in names
    assert "get_event_loop" in names
    assert "get_running_loop" in names
    assert checks == {"behavior": "True", "private_handles_hidden": "True"}
