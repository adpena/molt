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
    next_lock = 0
    locks = {{}}
    next_thread = 100
    stack_size = 0


def _lock_new():
    _State.next_lock += 1
    handle = _State.next_lock
    _State.locks[handle] = False
    return handle


def _lock_acquire(handle, blocking, timeout):
    _State.locks[handle] = True
    return True


def _lock_release(handle):
    _State.locks[handle] = False


def _lock_locked(handle):
    return _State.locks[handle]


def _lock_drop(handle):
    _State.locks.pop(handle, None)


def _thread_spawn_shared(token, fn, args, kwargs):
    _State.next_thread += 1
    fn(*args, **kwargs)
    return _State.next_thread


def _thread_stack_size_set(size):
    _State.stack_size = size
    return size


builtins._molt_intrinsics = {{
    "molt_lock_new": _lock_new,
    "molt_lock_acquire": _lock_acquire,
    "molt_lock_release": _lock_release,
    "molt_lock_locked": _lock_locked,
    "molt_lock_drop": _lock_drop,
    "molt_thread_spawn_shared": _thread_spawn_shared,
    "molt_thread_ident": lambda handle: handle,
    "molt_thread_current_ident": lambda: 1,
    "molt_thread_current_native_id": lambda: 11,
    "molt_thread_registry_active_count": lambda: 1,
    "molt_thread_stack_size_get": lambda: _State.stack_size,
    "molt_thread_stack_size_set": _thread_stack_size_set,
    "molt_signal_raise_signal": lambda signum: None,
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


_private = _load_module("_molt_private_thread", {str(STDLIB_ROOT / "_thread.py")!r})

rows = [
    (name, type(getattr(_private, name)).__name__, bool(callable(getattr(_private, name))))
    for name in sorted(dir(_private))
    if not name.startswith("_") and name != "annotations"
]
for name, type_name, is_callable in rows:
    print(f"ROW|{{name}}|{{type_name}}|{{is_callable}}")

box = []
lock = _private.allocate_lock()
lock.acquire()
lock.release()
thread_id = _private.start_new_thread(lambda bucket, value: bucket.append(value), (box, "ok"))
checks = {{
    "behavior": (
        type(lock).__name__ == "lock"
        and lock.locked() is False
        and thread_id == 101
        and box == ["ok"]
        and _private.get_ident() == 1
        and _private.get_native_id() == 11
        and _private.stack_size() == 0
        and _private.stack_size(4096) == 4096
    ),
    "private_handles_hidden": (
        "_lock_new" not in _private.__dict__
        and "_lock_acquire" not in _private.__dict__
        and "_lock_release" not in _private.__dict__
        and "_lock_locked" not in _private.__dict__
        and "_lock_drop" not in _private.__dict__
        and "_thread_spawn_shared" not in _private.__dict__
        and "_thread_ident" not in _private.__dict__
        and "_thread_current_ident" not in _private.__dict__
        and "_thread_current_native_id" not in _private.__dict__
        and "_thread_registry_active_count" not in _private.__dict__
        and "_thread_stack_size_get" not in _private.__dict__
        and "_thread_stack_size_set" not in _private.__dict__
        and "_signal_raise_signal" not in _private.__dict__
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


def test__thread_public_surface_matches_expected_shape() -> None:
    rows, checks = _run_probe()
    assert rows == [
        ("LockType", "type", "True"),
        ("TIMEOUT_MAX", "float", "False"),
        ("allocate", "function", "True"),
        ("allocate_lock", "function", "True"),
        ("error", "type", "True"),
        ("exit", "function", "True"),
        ("exit_thread", "function", "True"),
        ("get_ident", "function", "True"),
        ("get_native_id", "function", "True"),
        ("interrupt_main", "function", "True"),
        ("lock", "type", "True"),
        ("stack_size", "function", "True"),
        ("start_new", "function", "True"),
        ("start_new_thread", "function", "True"),
    ]
    assert checks == {"behavior": "True", "private_handles_hidden": "True"}
